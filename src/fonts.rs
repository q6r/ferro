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
    fn parse(data: &[u8]) -> Option<ColorTable> {
        let u16_at = |o: usize| -> Option<u16> { Some(u16::from_be_bytes([*data.get(o)?, *data.get(o + 1)?])) };
        let u32_at = |o: usize| -> Option<u32> {
            Some(u32::from_be_bytes([*data.get(o)?, *data.get(o + 1)?, *data.get(o + 2)?, *data.get(o + 3)?]))
        };
        // diretório de tabelas
        let n = u16_at(4)? as usize;
        let mut colr = None;
        let mut cpal = None;
        for i in 0..n {
            let rec = 12 + i * 16;
            let tag = data.get(rec..rec + 4)?;
            let off = u32_at(rec + 8)? as usize;
            match tag {
                b"COLR" => colr = Some(off),
                b"CPAL" => cpal = Some(off),
                _ => {}
            }
        }
        let (c, p) = (colr?, cpal?);
        let nbase = u16_at(c + 2)? as usize;
        let base_off = c + u32_at(c + 4)? as usize;
        let layer_off = c + u32_at(c + 8)? as usize;
        let nlayers = u16_at(c + 12)? as usize;
        let mut base = HashMap::with_capacity(nbase);
        for i in 0..nbase {
            let r = base_off + i * 6;
            base.insert(u16_at(r)?, (u16_at(r + 2)?, u16_at(r + 4)?));
        }
        let mut layers = Vec::with_capacity(nlayers);
        for i in 0..nlayers {
            let r = layer_off + i * 4;
            layers.push((u16_at(r)?, u16_at(r + 2)?));
        }
        let entries = u16_at(p + 2)? as usize;
        let records = p + u32_at(p + 8)? as usize;
        let first = u16_at(p + 12)? as usize;
        let mut palette = Vec::with_capacity(entries);
        for i in 0..entries {
            let r = records + (first + i) * 4;
            // BGRA no arquivo
            palette.push([*data.get(r + 2)?, *data.get(r + 1)?, *data.get(r)?, *data.get(r + 3)?]);
        }
        Some(ColorTable { base, layers, palette, data: data.to_vec() })
    }
}

/// Um emoji colorido rasterizado: RGBA não pré-multiplicado, com a mesma
/// posição de um glifo comum (`metrics`).
pub struct ColorGlyph {
    pub metrics: Metrics,
    pub rgba: Vec<u8>,
}

pub struct Fonts {
    faces: Vec<Face>,
    glyphs: RefCell<HashMap<(FontId, char, u32), Glyph>>,
    advances: RefCell<HashMap<(FontId, char, u32), f32>>,
    color_glyphs: RefCell<HashMap<(FontId, char, u32), Option<std::rc::Rc<ColorGlyph>>>>,
}

#[derive(Clone)]
pub struct Glyph {
    pub metrics: Metrics,
    pub coverage: std::rc::Rc<Vec<u8>>,
}

/// Métricas verticais de uma fonte num tamanho.
#[derive(Clone, Copy, Debug)]
pub struct LineMetrics {
    pub ascent: f32,
    pub descent: f32,
    /// Altura de linha "normal" (ascent + descent + line gap).
    pub normal: f32,
}

// Cada face lista caminhos em ordem de preferência (Windows, Linux, macOS).
const FACES: &[&[&str]] = &[
    // 0..4: sans (regular, bold, italic, bold italic)
    &["C:/Windows/Fonts/arial.ttf", "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/System/Library/Fonts/Supplemental/Arial.ttf"],
    &["C:/Windows/Fonts/arialbd.ttf", "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", "/System/Library/Fonts/Supplemental/Arial Bold.ttf"],
    &["C:/Windows/Fonts/ariali.ttf", "/usr/share/fonts/truetype/liberation/LiberationSans-Italic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf", "/System/Library/Fonts/Supplemental/Arial Italic.ttf"],
    &["C:/Windows/Fonts/arialbi.ttf", "/usr/share/fonts/truetype/liberation/LiberationSans-BoldItalic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf", "/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf"],
    // 4..8: serif
    &["C:/Windows/Fonts/times.ttf", "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf", "/System/Library/Fonts/Supplemental/Times New Roman.ttf"],
    &["C:/Windows/Fonts/timesbd.ttf", "/usr/share/fonts/truetype/liberation/LiberationSerif-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf", "/System/Library/Fonts/Supplemental/Times New Roman Bold.ttf"],
    &["C:/Windows/Fonts/timesi.ttf", "/usr/share/fonts/truetype/liberation/LiberationSerif-Italic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Italic.ttf", "/System/Library/Fonts/Supplemental/Times New Roman Italic.ttf"],
    &["C:/Windows/Fonts/timesbi.ttf", "/usr/share/fonts/truetype/liberation/LiberationSerif-BoldItalic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif-BoldItalic.ttf", "/System/Library/Fonts/Supplemental/Times New Roman Bold Italic.ttf"],
    // 8..12: mono
    &["C:/Windows/Fonts/consola.ttf", "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/System/Library/Fonts/Supplemental/Courier New.ttf"],
    &["C:/Windows/Fonts/consolab.ttf", "/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", "/System/Library/Fonts/Supplemental/Courier New Bold.ttf"],
    &["C:/Windows/Fonts/consolai.ttf", "/usr/share/fonts/truetype/liberation/LiberationMono-Italic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf", "/System/Library/Fonts/Supplemental/Courier New Italic.ttf"],
    &["C:/Windows/Fonts/consolaz.ttf", "/usr/share/fonts/truetype/liberation/LiberationMono-BoldItalic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-BoldOblique.ttf", "/System/Library/Fonts/Supplemental/Courier New Bold Italic.ttf"],
    // 12..16: fonte do sistema (system-ui), usada também na interface
    &["C:/Windows/Fonts/segoeui.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"],
    &["C:/Windows/Fonts/segoeuib.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"],
    &["C:/Windows/Fonts/segoeuii.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Italic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf"],
    &["C:/Windows/Fonts/segoeuiz.ttf", "/usr/share/fonts/truetype/noto/NotoSans-BoldItalic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf"],
    // 16: seminegrito; 17: ícones
    &["C:/Windows/Fonts/seguisb.ttf", "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf"],
    &["C:/Windows/Fonts/SegoeIcons.ttf", "C:/Windows/Fonts/segmdl2.ttf"],
    // 18..: fallback para caracteres que faltam (símbolos, setas, emoji em preto e branco)
    &["C:/Windows/Fonts/segoeui.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"],
    &["C:/Windows/Fonts/seguisym.ttf", "/usr/share/fonts/truetype/noto/NotoSansSymbols-Regular.ttf"],
    &["C:/Windows/Fonts/seguiemj.ttf", "/usr/share/fonts/truetype/noto/NotoEmoji-Regular.ttf"],
    // chinês, japonês e coreano (só carregam quando aparece um caractere desses)
    &["C:/Windows/Fonts/msyh.ttc", "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", "/System/Library/Fonts/PingFang.ttc"],
    &["C:/Windows/Fonts/YuGothM.ttc", "C:/Windows/Fonts/msgothic.ttc"],
    &["C:/Windows/Fonts/malgun.ttf"],
    &["C:/Windows/Fonts/Nirmala.ttc", "C:/Windows/Fonts/Nirmala.ttf"],
    &["C:/Windows/Fonts/seguihis.ttf", "C:/Windows/Fonts/ebrima.ttf"],
];
const SEMIBOLD: FontId = 16;
const ICONS: FontId = 17;
const FALLBACK_START: usize = 18;
/// Segoe UI Emoji / Noto Emoji.
const EMOJI: FontId = 20;

impl Fonts {
    pub fn system() -> Result<Fonts, String> {
        let fonts = Fonts {
            faces: FACES
                .iter()
                .map(|c| Face {
                    candidates: c,
                    font: OnceCell::new(),
                    colr: OnceCell::new(),
                })
                .collect(),
            glyphs: RefCell::new(HashMap::new()),
            advances: RefCell::new(HashMap::new()),
            color_glyphs: RefCell::new(HashMap::new()),
        };
        if fonts.face(0).is_none() && fonts.face(4).is_none() {
            return Err("nenhuma fonte do sistema encontrada (Arial/Times/DejaVu/Liberation)".into());
        }
        Ok(fonts)
    }

    fn face(&self, id: FontId) -> Option<FontRef<'_>> {
        if id >= WEB_FONT_BASE {
            let list = web_fonts().read().unwrap();
            return list.get(id - WEB_FONT_BASE).map(|w| FontRef::Web(w.font.clone()));
        }
        self.faces
            .get(id)?
            .font
            .get_or_init(|| {
                self.faces[id].candidates.iter().find_map(|p| {
                    let bytes = std::fs::read(PathBuf::from(p)).ok()?;
                    // emoji: guarda as camadas coloridas, se a fonte tiver
                    let _ = self.faces[id].colr.set(ColorTable::parse(&bytes));
                    Font::from_bytes(bytes, FontSettings::default()).ok()
                })
            })
            .as_ref()
            .map(FontRef::Sys)
    }

    /// Fonte de um estilo: primeiro as fontes web pelo nome da família,
    /// depois a família genérica do sistema.
    pub fn for_style(&self, s: &ComputedStyle) -> FontId {
        for name in s.font_names.iter() {
            if let Some(id) = find_web(name, s.font_weight, s.italic) {
                return id;
            }
        }
        self.pick(s.font_family, s.font_weight, s.italic)
    }

    /// Fonte web (`@font-face`) com esse nome de família, se já carregou.
    pub fn web_font(&self, family: &str, weight: u16, italic: bool) -> Option<FontId> {
        find_web(family, weight, italic)
    }

    /// Escolhe a face para um estilo; cai para sans regular se não existir.
    pub fn pick(&self, family: FontFamily, weight: u16, italic: bool) -> FontId {
        let base = match family {
            FontFamily::Sans => 0,
            FontFamily::Serif => 4,
            FontFamily::Mono => 8,
            FontFamily::System => 12,
        };
        if family == FontFamily::System && (550..700).contains(&weight) && !italic && self.face(SEMIBOLD).is_some() {
            return SEMIBOLD;
        }
        let id = base + (weight >= 600) as usize + 2 * italic as usize;
        [id, base, 0, 4].into_iter().find(|&i| self.face(i).is_some()).unwrap_or(0)
    }

    /// Fonte de ícones do sistema (Segoe Fluent Icons / MDL2), se existir.
    pub fn icons(&self) -> Option<FontId> {
        self.face(ICONS).map(|_| ICONS)
    }

