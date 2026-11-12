//! Motor: o pipeline completo de uma página (rede → DOM → CSS → layout) e a
//! thread de renderização de cada aba, que conversa com a interface por
//! mensagens, no espírito do processo de renderização do Chrome.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use url::Url;

use crate::css::{self, FontFaceRule, MediaContext};
use crate::dom::{Document, NodeId};
use crate::fonts::{self, Fonts};
use crate::layout::{self, Images, Page, Rect};
use crate::net::{self, Net};
use crate::paint::{DisplayItem, ImageData};
use crate::style::{self, Color, Origin};
use crate::{UA_STYLESHEET, html};

/// Folha de estilo do autor, ainda em texto (o atributo `media` é avaliado
/// a cada layout, porque depende da largura da janela).
pub struct StyleSource {
    pub text: String,
    pub media: Option<String>,
}

/// Documento carregado: DOM, folhas de estilo e URLs.
pub struct Loaded {
    pub url: Url,
    pub base: Url,
    pub status: u16,
    pub doc: Document,
    pub css: Vec<StyleSource>,
    pub title: String,
    /// Texto das folhas externas já baixadas (None = falhou), por URL.
    pub sheet_texts: HashMap<Url, Option<String>>,
}

impl Loaded {
    /// Título atual do documento (os scripts podem mudar).
    pub fn current_title(&self) -> String {
        self.doc
            .find_first("title")
            .map(|t| self.doc.text_content(t).split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Baixa o documento e as folhas de estilo dele.
pub fn load(net: &Net, url: &Url) -> Result<Loaded, String> {
    load_with(net, url, None)
}

/// Um arquivo que não é página: vai para a pasta de downloads.
pub struct Download {
    pub url: Url,
    pub name: String,
    pub bytes: Vec<u8>,
}

/// O que veio de uma navegação.
pub enum Fetched {
    Page(Box<Loaded>),
    Download(Download),
}

/// Nome do arquivo pelo `Content-Disposition` (filename* ou filename) ou pela URL.
fn download_name(res: &net::Response) -> String {
    let cd = res.headers.iter().find(|(k, _)| k == "content-disposition").map(|(_, v)| v.as_str()).unwrap_or("");
    let mut name = None;
    for part in cd.split(';') {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"'));
        if k == "filename*" {
            // UTF-8''nome%20codificado
            let enc = v.rsplit("''").next().unwrap_or(v);
            name = Some(String::from_utf8_lossy(&net::percent_decode(enc)).into_owned());
            break;
        }
        if k == "filename" && !v.is_empty() {
            name = Some(v.to_string());
        }
    }
    let name = name.unwrap_or_else(|| {
        let last = res.url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
        let last = String::from_utf8_lossy(&net::percent_decode(last)).into_owned();
        if last.is_empty() { "download".to_string() } else { last }
    });
    // sem caminhos nem caracteres proibidos no Windows
    let clean: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() { "download".into() } else { clean }
}

