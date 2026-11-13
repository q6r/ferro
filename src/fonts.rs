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
    /// Fonte variável com eixo de peso (wght): os bytes e o intervalo do eixo.
    var: Option<VarInfo>,
}

struct VarInfo {
    data: Arc<Vec<u8>>,
    min: f32,
    def: f32,
    max: f32,
}

/// Ids das instâncias de fontes variáveis: base + índice da fonte web × 1024 + peso.
const VAR_BASE: FontId = 1 << 24;

/// (fonte web de base, peso da instância) de um id.
fn var_split(id: FontId) -> (FontId, Option<u16>) {
    if id >= VAR_BASE {
        let k = id - VAR_BASE;
        (WEB_FONT_BASE + k / 1024, Some((k % 1024) as u16))
    } else {
        (id, None)
    }
}

/// Eixo de peso de uma fonte variável, se tiver.
fn wght_axis(data: &[u8]) -> Option<(f32, f32, f32)> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let tag = ttf_parser::Tag::from_bytes(b"wght");
    face.variation_axes().into_iter().find(|a| a.tag == tag).map(|a| (a.min_value, a.def_value, a.max_value))
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
    let var = wght_axis(&data).map(|(min, def, max)| VarInfo { data: Arc::new(data.clone()), min, def, max });
    let Ok(font) = Font::from_bytes(data, FontSettings::default()) else { return false };
    web_fonts().write().unwrap().push(WebFont {
        family: family.to_string(),
        weight,
        italic,
        font: Arc::new(font),
        var,
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
        .map(|(i, f)| match &f.var {
            // variável: uma instância no peso pedido (fora o padrão da fonte)
            Some(v) => {
                let w = (weight as f32).clamp(v.min, v.max).round();
                if (w - v.def).abs() >= 1.0 && (1.0..1024.0).contains(&w) {
                    VAR_BASE + i * 1024 + w as usize
                } else {
                    WEB_FONT_BASE + i
                }
            }
            None => WEB_FONT_BASE + i,
        })
}

/// Outra fonte web da mesma família que tenha o caractere (as fontes do
/// Google vêm divididas por alfabeto).
fn web_sibling(id: FontId, ch: char) -> Option<FontId> {
    let id = var_split(id).0;
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
        let id = var_split(id).0;
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

    /// Face que realmente tem o caractere (com fallback).
    fn resolve(&self, id: FontId, ch: char) -> FontId {
        if let Some(f) = self.face(id)
            && (f.lookup_glyph_index(ch) != 0 || ch.is_control())
        {
            return id;
        }
        if id >= WEB_FONT_BASE
            && let Some(sib) = web_sibling(id, ch)
        {
            return sib;
        }
        if ch.is_whitespace() && self.face(id).is_some() {
            return id;
        }
        // emoji: a fonte colorida antes da de símbolos (que tem versões em preto)
        if (ch as u32) >= 0x1F000 && self.face(EMOJI).is_some_and(|f| f.lookup_glyph_index(ch) != 0) {
            return EMOJI;
        }
        (FALLBACK_START..self.faces.len())
            .find(|&i| self.face(i).is_some_and(|f| f.lookup_glyph_index(ch) != 0))
            .unwrap_or(id)
    }

    fn key(size: f32) -> u32 {
        (size * 4.0).round() as u32
    }

    pub fn advance(&self, id: FontId, ch: char, size: f32) -> f32 {
        // seletores de variação e ZWJ não ocupam espaço
        if matches!(ch, '\u{FE00}'..='\u{FE0F}' | '\u{200D}' | '\u{200B}' | '\u{2060}') {
            return 0.0;
        }
        let k = (id, ch, Self::key(size));
        if let Some(a) = self.advances.borrow().get(&k) {
            return *a;
        }
        let rid = self.resolve(id, ch);
        let a = match self.var_advance(rid, ch, size) {
            Some(a) => a,
            None => self.face(rid).map(|f| f.metrics(ch, size).advance_width).unwrap_or(size * 0.5),
        };
        self.advances.borrow_mut().insert(k, a);
        a
    }

    pub fn measure(&self, id: FontId, size: f32, text: &str) -> f32 {
        text.chars().map(|c| self.advance(id, c, size)).sum()
    }

    pub fn line_metrics(&self, id: FontId, size: f32) -> LineMetrics {
        match self.face(id).and_then(|f| f.horizontal_line_metrics(size)) {
            Some(m) => LineMetrics {
                ascent: m.ascent,
                descent: -m.descent,
                normal: m.new_line_size,
            },
            None => LineMetrics {
                ascent: size * 0.9,
                descent: size * 0.25,
                normal: size * 1.15,
            },
        }
    }

    /// Emoji colorido (camadas COLR), se a face que tem o caractere for colorida.
    /// `emoji`: o caractere veio com U+FE0F (pede a versão emoji).
    pub fn color_glyph(&self, id: FontId, ch: char, size: f32, emoji: bool) -> Option<std::rc::Rc<ColorGlyph>> {
        if (ch as u32) < 0x2000 && !emoji {
            return None;
        }
        let rid = if emoji && self.face(EMOJI).is_some_and(|f| f.lookup_glyph_index(ch) != 0) { EMOJI } else { self.resolve(id, ch) };
        let k = (rid, ch, Self::key(size));
        if let Some(g) = self.color_glyphs.borrow().get(&k) {
            return g.clone();
        }
        let g = self.make_color_glyph(rid, ch, size).map(std::rc::Rc::new);
        self.color_glyphs.borrow_mut().insert(k, g.clone());
        g
    }

    fn make_color_glyph(&self, rid: FontId, ch: char, size: f32) -> Option<ColorGlyph> {
        use crate::canvas::{Bitmap, Cmd, Mat, Op, Paint, Style};
        if rid >= WEB_FONT_BASE {
            return None;
        }
        let font = self.face(rid)?;
        let table = self.faces.get(rid)?.colr.get()?.as_ref()?;
        let gid = font.lookup_glyph_index(ch);
        let &(first, n) = table.base.get(&gid)?;
        let face = ttf_parser::Face::parse(&table.data, 0).ok()?;
        let scale = size / face.units_per_em() as f32;
        let layers: Vec<(u16, [u8; 4])> = (first..first + n)
            .filter_map(|i| {
                let (g, pi) = *table.layers.get(i as usize)?;
                let color = if pi == 0xFFFF { [0, 0, 0, 255] } else { *table.palette.get(pi as usize)? };
                Some((g, color))
            })
            .collect();
        // caixa de todas as camadas, em unidades da fonte
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for (g, _) in &layers {
            if let Some(b) = face.glyph_bounding_box(ttf_parser::GlyphId(*g)) {
                x0 = x0.min(b.x_min as i32);
                y0 = y0.min(b.y_min as i32);
                x1 = x1.max(b.x_max as i32);
                y1 = y1.max(b.y_max as i32);
            }
        }
        if x0 > x1 {
            return None;
        }
        let px0 = (x0 as f32 * scale).floor();
        let py0 = (y0 as f32 * scale).floor();
        let w = ((x1 as f32 * scale).ceil() - px0).max(1.0) as usize + 1;
        let h = ((y1 as f32 * scale).ceil() - py0).max(1.0) as usize + 1;
        let top = py0 + h as f32;
        // contorno de cada camada → caminho em pixels (y para baixo)
        struct Path {
            cmds: Vec<Cmd>,
            last: (f32, f32),
            scale: f32,
            px0: f32,
            top: f32,
        }
        impl Path {
            fn p(&self, x: f32, y: f32) -> (f32, f32) {
                (x * self.scale - self.px0, self.top - y * self.scale)
            }
        }
        impl ttf_parser::OutlineBuilder for Path {
            fn move_to(&mut self, x: f32, y: f32) {
                let (a, b) = self.p(x, y);
                self.cmds.push(Cmd::Move(a, b));
                self.last = (a, b);
            }
            fn line_to(&mut self, x: f32, y: f32) {
                let (a, b) = self.p(x, y);
                self.cmds.push(Cmd::Line(a, b));
                self.last = (a, b);
            }
            fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
                let (cx, cy) = self.p(x1, y1);
                let (ex, ey) = self.p(x, y);
                let (sx, sy) = self.last;
                self.cmds.push(Cmd::Cubic(
                    sx + 2.0 / 3.0 * (cx - sx),
                    sy + 2.0 / 3.0 * (cy - sy),
                    ex + 2.0 / 3.0 * (cx - ex),
                    ey + 2.0 / 3.0 * (cy - ey),
                    ex,
                    ey,
                ));
                self.last = (ex, ey);
            }
            fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
                let (a, b) = self.p(x1, y1);
                let (c, d) = self.p(x2, y2);
                let (e, f) = self.p(x, y);
                self.cmds.push(Cmd::Cubic(a, b, c, d, e, f));
                self.last = (e, f);
            }
            fn close(&mut self) {
                self.cmds.push(Cmd::Close);
            }
        }
        let mut bmp = Bitmap::new(w, h);
        for (g, c) in &layers {
            let mut path = Path { cmds: Vec::new(), last: (0.0, 0.0), scale, px0, top };
            if face.outline_glyph(ttf_parser::GlyphId(*g), &mut path).is_none() {
                continue;
            }
            let color = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0];
            let paint = Paint { style: Style::Solid(color), inv: Mat::IDENTITY, alpha: 1.0, op: Op::SourceOver };
            bmp.fill_path(&path.cmds, false, &paint);
        }
        let img = bmp.snapshot();
        let adv = face.glyph_hor_advance(ttf_parser::GlyphId(gid)).map(|a| a as f32 * scale).unwrap_or(size);
        let base = font.metrics(ch, size);
        Some(ColorGlyph {
            metrics: Metrics { xmin: px0 as i32, ymin: py0 as i32, width: w, height: h, advance_width: adv, ..base },
            rgba: img.rgba,
        })
    }

    /// Bytes de uma instância variável (fonte, peso).
    fn var_face_data(id: FontId) -> Option<(Arc<Vec<u8>>, u16)> {
        let (base, Some(w)) = var_split(id) else { return None };
        let list = web_fonts().read().unwrap();
        let v = list.get(base - WEB_FONT_BASE)?.var.as_ref()?;
        Some((v.data.clone(), w))
    }

    /// Avanço numa instância variável (o HVAR entra pelo ttf-parser).
    fn var_advance(&self, id: FontId, ch: char, size: f32) -> Option<f32> {
        let (data, w) = Self::var_face_data(id)?;
        let mut face = ttf_parser::Face::parse(&data, 0).ok()?;
        face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), w as f32);
        let gid = face.glyph_index(ch)?;
        Some(face.glyph_hor_advance(gid)? as f32 * size / face.units_per_em() as f32)
    }

    /// Glifo de uma instância variável: o contorno com a variação aplicada,
    /// preenchido pelo rasterizador do canvas (o fontdue só desenha o padrão).
    fn var_glyph(&self, id: FontId, ch: char, size: f32) -> Option<(Metrics, Vec<u8>)> {
        use crate::canvas::{Bitmap, Cmd, Mat, Op, Paint, Style};
        let (data, w) = Self::var_face_data(id)?;
        let mut face = ttf_parser::Face::parse(&data, 0).ok()?;
        face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), w as f32);
        let gid = face.glyph_index(ch)?;
        let scale = size / face.units_per_em() as f32;
        let adv = face.glyph_hor_advance(gid).map(|a| a as f32 * scale).unwrap_or(size * 0.5);
        let base = self.face(id)?.metrics(ch, size);
        // contorno em unidades da fonte, guardando a caixa (a do glyf não varia)
        #[derive(Default)]
        struct Rec {
            segs: Vec<(u8, [f32; 6])>,
            x0: f32,
            y0: f32,
            x1: f32,
            y1: f32,
            any: bool,
        }
        impl Rec {
            fn pt(&mut self, x: f32, y: f32) {
                if !self.any {
                    (self.x0, self.y0, self.x1, self.y1, self.any) = (x, y, x, y, true);
                }
                self.x0 = self.x0.min(x);
                self.y0 = self.y0.min(y);
                self.x1 = self.x1.max(x);
                self.y1 = self.y1.max(y);
            }
        }
        impl ttf_parser::OutlineBuilder for Rec {
            fn move_to(&mut self, x: f32, y: f32) {
                self.pt(x, y);
                self.segs.push((0, [x, y, 0.0, 0.0, 0.0, 0.0]));
            }
            fn line_to(&mut self, x: f32, y: f32) {
                self.pt(x, y);
                self.segs.push((1, [x, y, 0.0, 0.0, 0.0, 0.0]));
            }
            fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
                self.pt(x1, y1);
                self.pt(x, y);
                self.segs.push((2, [x1, y1, x, y, 0.0, 0.0]));
            }
            fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
                self.pt(x1, y1);
                self.pt(x2, y2);
                self.pt(x, y);
                self.segs.push((3, [x1, y1, x2, y2, x, y]));
            }
            fn close(&mut self) {
                self.segs.push((4, [0.0; 6]));
            }
        }
        let mut rec = Rec::default();
        if face.outline_glyph(gid, &mut rec).is_none() || !rec.any {
            // sem contorno (espaço): só o avanço
            return Some((Metrics { xmin: 0, ymin: 0, width: 0, height: 0, advance_width: adv, ..base }, Vec::new()));
        }
        let px0 = (rec.x0 * scale).floor();
        let py0 = (rec.y0 * scale).floor();
        let wpx = ((rec.x1 * scale).ceil() - px0).max(1.0) as usize + 1;
        let hpx = ((rec.y1 * scale).ceil() - py0).max(1.0) as usize + 1;
        let top = py0 + hpx as f32;
        let p = |x: f32, y: f32| (x * scale - px0, top - y * scale);
        let mut cmds = Vec::with_capacity(rec.segs.len());
        let mut last = (0.0f32, 0.0f32);
        for (k, a) in &rec.segs {
            match k {
                0 => {
                    last = p(a[0], a[1]);
                    cmds.push(Cmd::Move(last.0, last.1));
                }
                1 => {
                    last = p(a[0], a[1]);
                    cmds.push(Cmd::Line(last.0, last.1));
                }
                2 => {
                    let (cx, cy) = p(a[0], a[1]);
                    let (ex, ey) = p(a[2], a[3]);
                    let (sx, sy) = last;
                    cmds.push(Cmd::Cubic(
                        sx + 2.0 / 3.0 * (cx - sx),
                        sy + 2.0 / 3.0 * (cy - sy),
                        ex + 2.0 / 3.0 * (cx - ex),
                        ey + 2.0 / 3.0 * (cy - ey),
                        ex,
                        ey,
                    ));
                    last = (ex, ey);
                }
                3 => {
                    let (b1, b2) = p(a[0], a[1]);
                    let (c1, c2) = p(a[2], a[3]);
                    let (e1, e2) = p(a[4], a[5]);
                    cmds.push(Cmd::Cubic(b1, b2, c1, c2, e1, e2));
                    last = (e1, e2);
                }
                _ => cmds.push(Cmd::Close),
            }
        }
        let mut bmp = Bitmap::new(wpx, hpx);
        let paint = Paint { style: Style::Solid([1.0, 1.0, 1.0, 1.0]), inv: Mat::IDENTITY, alpha: 1.0, op: Op::SourceOver };
        bmp.fill_path(&cmds, false, &paint);
        let img = bmp.snapshot();
        let coverage: Vec<u8> = img.rgba.chunks(4).map(|px| px[3]).collect();
        Some((Metrics { xmin: px0 as i32, ymin: py0 as i32, width: wpx, height: hpx, advance_width: adv, ..base }, coverage))
    }

    pub fn glyph(&self, id: FontId, ch: char, size: f32) -> Option<Glyph> {
        let k = (id, ch, Self::key(size));
        if let Some(g) = self.glyphs.borrow().get(&k) {
            return Some(g.clone());
        }
        let rid = self.resolve(id, ch);
        let (metrics, coverage) = match self.var_glyph(rid, ch, size) {
            Some(g) => g,
            None => self.face(rid)?.rasterize(ch, size),
        };
        let g = Glyph {
            metrics,
            coverage: std::rc::Rc::new(coverage),
        };
        self.glyphs.borrow_mut().insert(k, g.clone());
        Some(g)
    }
}

#[cfg(test)]
mod tests_emoji {
    use super::*;

    #[test]
    fn emoji_colorido() {
        let f = Fonts::system().unwrap();
        let id = f.pick(FontFamily::Sans, 400, false);
        let cg = f.color_glyph(id, '😀', 32.0, false);
        assert!(cg.is_some_and(|g| g.rgba.chunks(4).any(|p| p[3] > 0 && p[0] > 100)));
    }
}
