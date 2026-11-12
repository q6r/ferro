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

/// Melhor fonte web da família para o peso e o estilo pedidos.
fn find_web(name: &str, weight: u16, italic: bool) -> Option<FontId> {
    let list = web_fonts().read().unwrap();
    list.iter()
        .enumerate()
        .filter(|(_, f)| f.family == name)
        .min_by_key(|(_, f)| {
            let dist = if weight < f.weight.0 {
                f.weight.0 - weight
            } else {
                weight.saturating_sub(f.weight.1)
            };
            (f.italic != italic) as u32 * 1000 + dist as u32
        })
        .map(|(i, _)| WEB_FONT_BASE + i)
}

/// Outra fonte web da mesma família que tenha o caractere (as fontes do
/// Google vêm divididas por alfabeto).
fn web_sibling(id: FontId, ch: char) -> Option<FontId> {
    let list = web_fonts().read().unwrap();
    let me = list.get(id - WEB_FONT_BASE)?;
    list.iter()
        .enumerate()
        .filter(|(_, f)| f.family == me.family && f.italic == me.italic && f.font.lookup_glyph_index(ch) != 0)
        .min_by_key(|(_, f)| f.weight.0.abs_diff(me.weight.0))
        .map(|(i, _)| WEB_FONT_BASE + i)
}

/// Referência a uma fonte do sistema (emprestada) ou web (compartilhada).
enum FontRef<'a> {
    Sys(&'a Font),
    Web(Arc<Font>),
}

impl Deref for FontRef<'_> {
    type Target = Font;

    fn deref(&self) -> &Font {
        match self {
            FontRef::Sys(f) => f,
            FontRef::Web(f) => f,
        }
    }
}

struct Face {
    candidates: &'static [&'static str],
    font: OnceCell<Option<Font>>,
    /// Camadas coloridas (tabelas COLR/CPAL), para emoji.
    colr: OnceCell<Option<ColorTable>>,
}

/// Glifos coloridos (COLR v0): cada glifo base vira camadas de outros
/// glifos, cada uma com uma cor da paleta (CPAL).
struct ColorTable {
    /// glifo base → (primeira camada, quantas)
    base: HashMap<u16, (u16, u16)>,
    /// (glifo, índice na paleta; 0xFFFF = cor do texto)
    layers: Vec<(u16, u16)>,
    /// Paleta 0, em RGBA.
    palette: Vec<[u8; 4]>,
    /// O arquivo da fonte (para os contornos das camadas, que o fontdue não carrega).
    data: Vec<u8>,
}

impl ColorTable {
