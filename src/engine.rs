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

/// A resposta deve virar download (e não página)?
fn is_download(res: &net::Response) -> bool {
    let cd = res.headers.iter().find(|(k, _)| k == "content-disposition").map(|(_, v)| v.to_ascii_lowercase());
    if cd.is_some_and(|v| v.trim_start().starts_with("attachment")) {
        return true;
    }
    let ct = res.content_type.as_deref().unwrap_or("").to_ascii_lowercase();
    let ct = ct.split(';').next().unwrap_or("").trim().to_string();
    if ct.is_empty() {
        // sem tipo: texto vira página, binário vira download
        return res.body.iter().take(512).any(|&b| b == 0);
    }
    !(ct.starts_with("text/")
        || ct.starts_with("image/")
        || ct.contains("html")
        || ct.contains("xml")
        || ct.contains("json")
        || ct.contains("javascript"))
}

/// Salva um download na pasta Downloads do usuário (sem sobrescrever nada).
pub fn save_download(d: &Download) -> Result<std::path::PathBuf, String> {
    let dir = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| std::path::PathBuf::from(h).join("Downloads"))
        .ok_or("pasta de downloads não encontrada")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (stem, ext) = match d.name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (d.name.clone(), String::new()),
    };
    let mut path = dir.join(&d.name);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&path, &d.bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Busca uma navegação: página ou download.
pub fn fetch_page(net: &Net, url: &Url, post: Option<&crate::script::PostData>) -> Result<Fetched, String> {
    let res = request_page(net, url, post)?;
    if res.status < 400 && is_download(&res) {
        let name = download_name(&res);
        return Ok(Fetched::Download(Download { url: res.url, name, bytes: res.body }));
    }
    Ok(Fetched::Page(Box::new(page_from_response(net, res))))
}

fn request_page(net: &Net, url: &Url, post: Option<&crate::script::PostData>) -> Result<net::Response, String> {
    Ok(match post {
        None => net.fetch(url)?,
        Some((ct, body)) => {
            let mut req = net::Request::get(url);
            req.method = "POST".into();
            req.headers.push(("Content-Type".into(), ct.clone()));
            req.headers.push(("Origin".into(), url.origin().ascii_serialization()));
            req.body = Some(body.clone());
            net.request(&req)?
        }
    })
}

/// Documento de um `<iframe>`: vai com o `Referer` da página que o contém
/// (players embutidos, como o do YouTube, exigem).
pub fn load_frame(net: &Net, url: &Url, referrer: Option<&Url>) -> Result<Loaded, String> {
    let mut req = net::Request::get(url);
    if let Some(r) = referrer.filter(|r| matches!(r.scheme(), "http" | "https")) {
        // só a origem quando muda de site (política padrão dos navegadores)
        let value = if r.origin() == url.origin() { r.to_string() } else { format!("{}/", r.origin().ascii_serialization()) };
        req.headers.push(("Referer".into(), value));
    }
    let res = net.request(&req)?;
    Ok(page_from_response(net, res))
}

/// Como `load`, opcionalmente enviando um formulário por POST.
pub fn load_with(net: &Net, url: &Url, post: Option<&crate::script::PostData>) -> Result<Loaded, String> {
    let res = request_page(net, url, post)?;
    Ok(page_from_response(net, res))
}

