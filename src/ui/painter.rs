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

