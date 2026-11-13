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
    let mut doc = crate::html::parse(html);
    crate::html::attach_declarative_shadow_roots(&mut doc);
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

    /// Uma volta nos quadros: scripts, timers, mensagens, `load` e o desenho.
    pub(crate) fn tick_frames(&mut self) {
        let mut frames = std::mem::take(&mut self.host.borrow_mut().frames);
        if frames.is_empty() {
            return;
        }
        let mut loaded = Vec::new();
        let mut messages: Vec<(NodeId, Message)> = Vec::new();
        let mut navigations = Vec::new();
        for (&n, c) in frames.iter_mut() {
            let s = &mut c.session;
            if s.scripts_pending() {
                s.step_scripts(std::time::Duration::from_millis(20));
            }
            s.tick();
            if !c.loaded && !s.scripts_pending() && s.idle() {
                s.finish_load();
                c.loaded = true;
                loaded.push(n);
            }
            for m in std::mem::take(&mut s.host.borrow_mut().outbox) {
                messages.push((n, m));
            }
            if let Some(nav) = s.take_navigation() {
                navigations.push((n, nav.url));
            }
            if s.wants_frame() {
                c.stale = true;
            }
        }
        // devolve os quadros (os eventos abaixo podem mexer no documento)
        {
            let mut hb = self.host.borrow_mut();
            let added = std::mem::replace(&mut hb.frames, frames);
            hb.frames.extend(added);
        }
        for n in loaded {
            let el = wrap(&mut self.js.vm, n);
            self.hook("fire", vec![el, Value::str("load")]);
        }
        for (n, (data, origin, target)) in messages {
            let mine = self.host.borrow().page.url.origin().ascii_serialization();
            if target != "*" && target != mine && !target.starts_with(&mine) {
                continue;
            }
            let el = wrap(&mut self.js.vm, n);
            self.hook("frameMessage", vec![el, Value::str(&data), Value::str(&origin)]);
        }
        // link clicado dentro do quadro: o quadro troca de página
        for (n, url) in navigations {
            let mut hb = self.host.borrow_mut();
            hb.frames.remove(&n);
            hb.frame_loading.insert(n, url.clone());
            let net = hb.net.clone();
            let referrer = hb.page.url.clone();
            hb.in_flight += 1;
            hb.pool.submit(Box::new(move || {
                let result = crate::engine::load_frame(&net, &url, Some(&referrer)).map(Box::new);
                Done::Frame { node: n, url, result }
            }));
        }
        self.paint_frames(false);
    }

    /// Redesenha as imagens dos quadros que mudaram (ou todas, com `all`).
    pub(crate) fn paint_frames(&mut self, all: bool) {
        let mut frames = std::mem::take(&mut self.host.borrow_mut().frames);
        let mut images = Vec::new();
        for (&n, c) in frames.iter_mut() {
            // a caixa do <iframe> pode ter mudado
            let size = frame_box(&self.host.borrow(), n).map(|r| (r.w.max(1.0), r.h.max(1.0)));
            if let Some(sz) = size
                && ((sz.0 - c.size.0).abs() > 0.5 || (sz.1 - c.size.1).abs() > 0.5)
            {
                c.size = sz;
                c.session.resize(sz.0, sz.1);
                c.stale = true;
            }
            if !(c.stale || all) {
                continue;
            }
            c.stale = false;
            let page = c.session.render();
            c.height = page.height;
            c.scroll = c.scroll.clamp(0.0, (page.height - c.size.1).max(0.0));
            let (w, h) = (c.size.0.ceil() as usize, c.size.1.ceil() as usize);
            if w == 0 || h == 0 || w * h > 4096 * 4096 {
                continue;
            }
            let mut canvas = Canvas::new(w, h, page.background);
            canvas.videos = c.session.video_frames();
            let t = Transform { scale: 1.0, dx: 0.0, dy: -c.scroll };
            canvas.paint_list(&page.items, &c.session.host.borrow().fonts.clone(), t, None);
            let mut rgba = Vec::with_capacity(w * h * 4);
            for p in &canvas.pixels {
                rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
            }
            images.push((n, Arc::new(ImageData { width: w as u32, height: h as u32, rgba })));
        }
        let mut hb = self.host.borrow_mut();
        let added = std::mem::replace(&mut hb.frames, frames);
        hb.frames.extend(added);
        for (n, img) in images {
            hb.frame_images.insert(n, img);
            hb.frames_changed.insert(n);
        }
    }

    /// O ponto (documento) cai sobre um quadro? Devolve o nó e o ponto no quadro.
    pub(crate) fn frame_at(&self, target: NodeId, x: f32, y: f32) -> Option<(NodeId, f32, f32)> {
        let hb = self.host.borrow();
        let c = hb.frames.get(&target)?;
        let r = frame_box(&hb, target)?;
        let fixed = hb.layout.as_ref().is_some_and(|l| l.fixed.contains(&target));
        let y = if fixed { y - hb.scroll_y } else { y };
        Some((target, x - r.x, y - r.y + c.scroll))
    }

    /// Roda do mouse sobre um quadro. `false` se não há quadro ou ele não rola.
    pub(crate) fn scroll_frame(&mut self, n: NodeId, dy: f32) -> bool {
        let Some(mut c) = self.host.borrow_mut().frames.remove(&n) else { return false };
        let max = (c.height - c.size.1).max(0.0);
        let next = (c.scroll + dy).clamp(0.0, max);
        let moved = (next - c.scroll).abs() >= 0.5;
        if moved {
            c.scroll = next;
            c.stale = true;
            // o quadro fica sabendo da rolagem (window.scrollY, eventos)
            c.session.set_scroll(next);
        }
        self.host.borrow_mut().frames.insert(n, c);
        if moved {
            self.paint_frames(false);
        }
        moved
    }

    /// Mensagens que chegaram da página-mãe: viram eventos `message`.
    pub(crate) fn deliver_inbox(&mut self) {
        let list = std::mem::take(&mut self.host.borrow_mut().inbox);
        let mine = self.host.borrow().page.url.origin().ascii_serialization();
        for (data, origin, target) in list {
            if target != "*" && target != mine && !target.starts_with(&mine) && mine != "null" {
                continue;
            }
            self.hook("parentMessage", vec![Value::str(&data), Value::str(&origin)]);
        }
    }
}

/// Caixa do `<iframe>` no layout (coordenadas do documento).
pub(crate) fn frame_box(hb: &Host, n: NodeId) -> Option<Rect> {
    let info = hb.layout.as_ref()?;
    info.boxes.iter().rev().find(|(id, _)| *id == n).map(|(_, r)| *r)
}

