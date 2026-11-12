//! `<iframe>`: cada quadro é uma sessão própria (documento, JS e layout
//! separados), dona da página que o contém.
//!
//! - Carrega pela rede (ou `srcdoc`/`about:blank`), roda os scripts aos
//!   poucos e desenha numa imagem do tamanho da caixa do `<iframe>`; a página
//!   mostra essa imagem como mostra um `<canvas>`.
//! - Cliques, mouse, teclas e roda do mouse sobre o `<iframe>` vão para o
//!   quadro, em coordenadas dele.
//! - Entre página e quadro só passa `postMessage` (como entre origens
//!   diferentes): os dados vão serializados em JSON.

use std::sync::Arc;

use ferro_js::Value;
use url::Url;

use super::{Done, Host, Session, wrap};
use crate::dom::NodeId;
use crate::engine::Loaded;
use crate::layout::Rect;
use crate::paint::{Canvas, ImageData, Transform};

/// Quantos quadros uma página pode ter (e quão fundo).
const MAX_FRAMES: usize = 12;
const MAX_DEPTH: u32 = 2;

pub(crate) struct Child {
    pub session: Box<Session>,
    pub url: Url,
    pub size: (f32, f32),
    /// Já disparou `load` no `<iframe>`.
    pub loaded: bool,
    /// Precisa redesenhar a imagem.
    pub stale: bool,
    pub scroll: f32,
    pub height: f32,
}

/// Mensagem entre quadros: (dados em JSON, origem de quem mandou, origem pedida).
pub(crate) type Message = (String, String, String);

/// O documento vazio de `about:blank` ou o de `srcdoc`.
fn inline_page(base: &Url, html: &str) -> Loaded {
    let url = Url::parse("about:blank").unwrap();
    let doc = crate::html::parse(html);
    Loaded {
        url,
        base: base.clone(),
        status: 200,
        doc,
        css: Vec::new(),
        title: String::new(),
        sheet_texts: Default::default(),
    }
}

impl Session {
    /// Procura `<iframe>`s novos (ou que trocaram de endereço) e começa a carregá-los.
    pub(crate) fn discover_frames(&mut self) {
        let (depth, found) = {
            let hb = self.host.borrow();
            let doc = &hb.page.doc;
            let mut found = Vec::new();
            for n in doc.find_all("iframe") {
                if !doc.is_connected(n) {
                    continue;
                }
                let Some(el) = doc.element(n) else { continue };
                let srcdoc = el.attr("srcdoc").map(str::to_string);
                let src = el.attr("src").map(|s| s.trim().to_string()).unwrap_or_default();
                let url = if srcdoc.is_some() || src.is_empty() || src == "about:blank" {
                    None
                } else {
                    match hb.page.base.join(&src) {
                        Ok(u) if matches!(u.scheme(), "http" | "https" | "data" | "file") => Some(u),
                        _ => continue,
                    }
                };
                let known = hb.frames.get(&n).map(|c| c.url.clone());
                let want = url.clone().unwrap_or_else(|| Url::parse("about:blank").unwrap());
                if known.as_ref() == Some(&want) || hb.frame_loading.get(&n) == Some(&want) {
                    continue;
                }
                found.push((n, url, srcdoc));
            }
            (hb.frame_depth, found)
        };
        if depth >= MAX_DEPTH {
            return;
        }
        for (n, url, srcdoc) in found {
            let count = {
                let hb = self.host.borrow();
                hb.frames.len() + hb.frame_loading.len()
            };
            if count >= MAX_FRAMES {
                break;
            }
            match url {
                Some(u) => {
                    let mut hb = self.host.borrow_mut();
                    hb.frame_loading.insert(n, u.clone());
                    let net = hb.net.clone();
                    let referrer = hb.page.url.clone();
                    hb.in_flight += 1;
                    hb.pool.submit(Box::new(move || {
                        let result = crate::engine::load_frame(&net, &u, Some(&referrer)).map(Box::new);
                        Done::Frame { node: n, url: u, result }
                    }));
                }
                None => {
                    let base = self.host.borrow().page.base.clone();
                    let page = inline_page(&base, srcdoc.as_deref().unwrap_or(""));
                    self.attach_frame(n, page);
                }
            }
        }
    }

    /// Um quadro chegou: cria a sessão dele.
    pub(crate) fn attach_frame(&mut self, n: NodeId, page: Loaded) {
        let (net, fonts, size, depth, origin) = {
            let mut hb = self.host.borrow_mut();
            hb.frame_loading.remove(&n);
            let size = frame_box(&hb, n).map_or((300.0, 150.0), |r| (r.w.max(1.0), r.h.max(1.0)));
            (hb.net.clone(), hb.fonts.clone(), size, hb.frame_depth, hb.page.url.origin().ascii_serialization())
        };
        let url = page.url.clone();
        let mut child = Session::new_frame(page, net, fonts, size.0, size.1, depth + 1, origin);
        // as <style> do documento (o carregamento pela rede já juntou as dele)
        child.host.borrow_mut().styles_dirty = true;
        child.start_scripts();
        let mut hb = self.host.borrow_mut();
        hb.frames.insert(n, Child { session: Box::new(child), url, size, loaded: false, stale: true, scroll: 0.0, height: size.1 });
        hb.dirty = true;
    }

