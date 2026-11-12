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

