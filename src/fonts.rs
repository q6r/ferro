//! Fontes do sistema: escolha da face (família, negrito, itálico), medição
//! de texto e cache de glifos rasterizados. A rasterização das curvas é do
//! fontdue; a escolha de fontes, o fallback e as métricas de linha são nossos.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use fontdue::{Font, FontSettings, Metrics};

use crate::style::{ComputedStyle, FontFamily};

pub type FontId = usize;

// ------------------------------------------------------------- fontes web

/// Ids a partir daqui são fontes web (do catálogo global).
pub const WEB_FONT_BASE: FontId = 1 << 16;

struct WebFont {
    family: String,
    weight: (u16, u16),
    italic: bool,
    font: Arc<Font>,
}

/// Catálogo de fontes web, compartilhado entre as threads: o motor mede o
/// texto numa thread e a interface desenha na outra, com os mesmos ids.
fn web_fonts() -> &'static RwLock<Vec<WebFont>> {
    static R: OnceLock<RwLock<Vec<WebFont>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(Vec::new()))
}

fn tried_urls() -> &'static Mutex<HashSet<String>> {
    static T: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Já tentamos baixar esta fonte (com ou sem sucesso)?
pub fn web_font_tried(url: &str) -> bool {
    tried_urls().lock().unwrap().contains(url)
}

/// Decodifica (TTF, OTF, WOFF ou WOFF2) e registra uma fonte web.
pub fn register_web_font(url: &str, family: &str, weight: (u16, u16), italic: bool, bytes: &[u8]) -> bool {
    tried_urls().lock().unwrap().insert(url.to_string());
    let data = if bytes.starts_with(b"wOF2") {
        match wuff::decompress_woff2(bytes) {
            Ok(d) => d,
            Err(_) => return false,
        }
    } else if bytes.starts_with(b"wOFF") {
        match wuff::decompress_woff1(bytes) {
            Ok(d) => d,
            Err(_) => return false,
        }
    } else {
        bytes.to_vec()
    };
    let Ok(font) = Font::from_bytes(data, FontSettings::default()) else { return false };
    web_fonts().write().unwrap().push(WebFont {
        family: family.to_string(),
        weight,
        italic,
        font: Arc::new(font),
    });
    true
}

