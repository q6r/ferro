//! Pincel da interface: desenha em coordenadas lógicas (DIP) e converte
//! para pixels do dispositivo pela escala de DPI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::fonts::{FontId, Fonts};
use crate::layout::Rect;
use crate::paint::{Canvas, ImageData};
use crate::style::{Color, FontFamily};
use crate::svg::{self, Svg};

#[derive(Clone, Copy, PartialEq)]
pub enum Weight {
    Regular,
    Semibold,
    Bold,
}

/// Ícones da interface: Mage Icons (estilo "stroke"), embutidos no binário
/// e desenhados pelo renderizador de SVG do próprio Ferro.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Back,
    Forward,
    Reload,
    Close,
    Plus,
    Search,
    Lock,
    Unlocked,
    Globe,
    Settings,
    Info,
    Swatch,
    Clock,
    ArrowUpRight,
    ChevronRight,
    ChevronLeft,
    Warning,
    Play,
    File,
    Home,
    Panel,
    Download,
}

impl Icon {
    fn source(self) -> &'static str {
        match self {
            Icon::Back => include_str!("../../assets/icons/mage/arrow-left.svg"),
            Icon::Forward => include_str!("../../assets/icons/mage/arrow-right.svg"),
            Icon::Reload => include_str!("../../assets/icons/mage/reload.svg"),
            Icon::Close => include_str!("../../assets/icons/mage/close.svg"),
            Icon::Plus => include_str!("../../assets/icons/mage/plus.svg"),
            Icon::Search => include_str!("../../assets/icons/mage/search.svg"),
            Icon::Lock => include_str!("../../assets/icons/mage/lock.svg"),
            Icon::Unlocked => include_str!("../../assets/icons/mage/unlocked.svg"),
            Icon::Globe => include_str!("../../assets/icons/mage/globe.svg"),
            Icon::Settings => include_str!("../../assets/icons/mage/settings.svg"),
            Icon::Info => include_str!("../../assets/icons/mage/info.svg"),
            Icon::Swatch => include_str!("../../assets/icons/mage/swatch.svg"),
            Icon::Clock => include_str!("../../assets/icons/mage/clock.svg"),
            Icon::ArrowUpRight => include_str!("../../assets/icons/mage/arrow-up-right.svg"),
            Icon::ChevronRight => include_str!("../../assets/icons/mage/chevron-right.svg"),
            Icon::ChevronLeft => include_str!("../../assets/icons/mage/chevron-left.svg"),
            Icon::Warning => include_str!("../../assets/icons/mage/warning.svg"),
            Icon::Play => include_str!("../../assets/icons/mage/play.svg"),
            Icon::File => include_str!("../../assets/icons/mage/file.svg"),
            Icon::Home => include_str!("../../assets/icons/mage/home.svg"),
            Icon::Panel => include_str!("../../assets/icons/mage/layout-left.svg"),
            Icon::Download => include_str!("../../assets/icons/mage/download.svg"),
        }
    }
}

/// Ícones lidos e rasterizados, por tamanho em pixels e cor.
#[derive(Default)]
pub struct IconCache {
    parsed: RefCell<HashMap<Icon, Option<Rc<Svg>>>>,
    raster: RefCell<HashMap<(Icon, u32, u32), Rc<ImageData>>>,
}

impl IconCache {
    pub fn get(&self, icon: Icon, px: u32, color: Color) -> Option<Rc<ImageData>> {
        let c = (color.r as u32) << 24 | (color.g as u32) << 16 | (color.b as u32) << 8 | color.a as u32;
        let key = (icon, px, c);
        if let Some(img) = self.raster.borrow().get(&key) {
            return Some(img.clone());
        }
        let svg = self
            .parsed
            .borrow_mut()
            .entry(icon)
            .or_insert_with(|| svg::parse(icon.source()).map(Rc::new))
            .clone()?;
        let img = Rc::new(svg.render(px, px, Some(color)));
        self.raster.borrow_mut().insert(key, img.clone());
        Some(img)
    }
}

pub struct Painter<'a> {
    pub canvas: &'a mut Canvas,
    pub fonts: &'a Fonts,
    pub icons: &'a IconCache,
    pub scale: f32,
    /// Recorte aplicado a tudo (coordenadas lógicas), p.ex. o painel da página.
    pub clip: Option<Rect>,
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    Rect {
        x: x0,
        y: y0,
        w: (x1 - x0).max(0.0),
        h: (y1 - y0).max(0.0),
    }
}

impl Painter<'_> {
    /// Recorte final em pixels: o global combinado com um específico.
    fn cut(&self, extra: Option<Rect>) -> Option<Rect> {
        let c = match (self.clip, extra) {
            (Some(a), Some(b)) => Some(intersect(a, b)),
            (a, b) => a.or(b),
        };
        c.map(|r| self.dev(r))
    }

    fn dev(&self, r: Rect) -> Rect {
        Rect {
            x: r.x * self.scale,
            y: r.y * self.scale,
            w: r.w * self.scale,
            h: r.h * self.scale,
        }
    }

    pub fn font(&self, weight: Weight) -> FontId {
        let w = match weight {
            Weight::Regular => 400,
            Weight::Semibold => 600,
            Weight::Bold => 700,
        };
        self.fonts.pick(FontFamily::System, w, false)
    }

    pub fn rect(&mut self, r: Rect, color: Color) {
        let (d, c) = (self.dev(r), self.cut(None));
        self.canvas.fill_rect(d, color, c);
    }

    pub fn rrect(&mut self, r: Rect, radius: f32, color: Color) {
        let (d, c) = (self.dev(r), self.cut(None));
        self.canvas.fill_round_rect(d, radius * self.scale, color, c);
    }

    pub fn rrect_clip(&mut self, r: Rect, radius: f32, color: Color, clip: Rect) {
        let (d, c) = (self.dev(r), self.cut(Some(clip)));
        self.canvas.fill_round_rect(d, radius * self.scale, color, c);
    }

    pub fn stroke(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        let (d, c) = (self.dev(r), self.cut(None));
        self.canvas
            .stroke_round_rect(d, radius * self.scale, (width * self.scale).max(1.0), color, c);
    }

    /// Sombra suave: camadas translúcidas crescentes atrás de um cartão.
    pub fn shadow(&mut self, r: Rect, radius: f32, strength: u8) {
        for i in 1..=6 {
            let g = i as f32 * 2.0;
            let rr = Rect {
                x: r.x - g / 2.0,
                y: r.y - g / 2.0 + 3.0,
                w: r.w + g,
                h: r.h + g,
            };
            self.rrect(
                rr,
                radius + g / 2.0,
                Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: strength / (i as u8 + 1),
                },
            );
        }
    }

    pub fn measure(&self, text: &str, size: f32, weight: Weight) -> f32 {
        self.fonts.measure(self.font(weight), size * self.scale, text) / self.scale
    }

    pub fn text(&mut self, x: f32, baseline: f32, text: &str, size: f32, color: Color, weight: Weight) {
        self.text_clipped(x, baseline, text, size, color, weight, None);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text_clipped(
        &mut self,
        x: f32,
        baseline: f32,
        text: &str,
        size: f32,
        color: Color,
        weight: Weight,
        clip: Option<Rect>,
    ) {
        let font = self.font(weight);
        let clip = self.cut(clip);
        self.canvas.draw_text(
            self.fonts,
            x * self.scale,
            baseline * self.scale,
            text,
            font,
            size * self.scale,
            color,
            clip,
        );
    }

    /// Linha de base para centralizar texto verticalmente em `cy`.
    pub fn baseline(cy: f32, size: f32) -> f32 {
        cy + size * 0.36
    }

    /// Texto que não cabe em `max_w` termina em reticências.
    pub fn ellipsize(&self, text: &str, size: f32, weight: Weight, max_w: f32) -> String {
        if self.measure(text, size, weight) <= max_w {
            return text.to_string();
        }
        let ell = self.measure("…", size, weight);
        let mut out = String::new();
        let mut w = 0.0;
        for c in text.chars() {
            let cw = self.measure(c.encode_utf8(&mut [0; 4]), size, weight);
            if w + cw + ell > max_w {
                break;
            }
            out.push(c);
            w += cw;
        }
        out.push('…');
        out
    }

    /// Ícone de `size` pixels lógicos, centralizado em (cx, cy).
    pub fn icon(&mut self, cx: f32, cy: f32, icon: Icon, size: f32, color: Color) {
        let px = (size * self.scale).round().max(4.0) as u32;
        let Some(img) = self.icons.get(icon, px, color) else { return };
        let r = Rect {
            x: (cx * self.scale - px as f32 / 2.0).round(),
            y: (cy * self.scale - px as f32 / 2.0).round(),
            w: px as f32,
            h: px as f32,
        };
        let c = self.cut(None);
        self.canvas.draw_image(&img, r, c);
    }
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

impl Painter<'_> {
    pub fn rrect_grad(&mut self, r: Rect, radius: f32, top: Color, bottom: Color) {
        let d = self.dev(r);
        self.canvas.fill_round_rect_gradient(d, radius * self.scale, top, bottom);
    }

    /// Desfoca o que está atrás de `r` (vidro fosco).
    pub fn frost(&mut self, r: Rect, radius: f32, blur: f32) {
        let d = self.dev(r);
        self.canvas.frost(d, radius * self.scale, blur * self.scale);
    }

    /// Arredonda os cantos do que já foi desenhado em `r`.
    pub fn round_corners(&mut self, r: Rect, radius: f32, outside: Color) {
        let d = self.dev(r);
        self.canvas.round_corners(d, radius * self.scale, outside);
    }

    /// Marca do Ferro: um "F" geométrico de barras arredondadas, com um
    /// quadradinho de destaque (no espírito do logo do kit).
    pub fn logo(&mut self, x: f32, y: f32, size: f32, bg: Color, fg: Color, spark: Color) {
        self.rrect(rect(x, y, size, size), size * 0.28, bg);
        let u = size / 10.0;
        let bar = u * 0.55;
        self.rrect(rect(x + 2.8 * u, y + 2.4 * u, 1.5 * u, 5.2 * u), bar, fg);
        self.rrect(rect(x + 2.8 * u, y + 2.4 * u, 4.6 * u, 1.5 * u), bar, fg);
        self.rrect(rect(x + 2.8 * u, y + 4.6 * u, 3.4 * u, 1.4 * u), bar, fg);
        self.rrect(rect(x + 6.0 * u, y + 6.1 * u, 1.5 * u, 1.5 * u), bar, spark);
    }
}
