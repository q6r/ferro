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
