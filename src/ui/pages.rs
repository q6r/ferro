//! Páginas internas desenhadas pela própria interface: nova aba,
//! configurações e erro de carregamento.

use crate::layout::Rect;
use crate::style::Color;

use super::painter::{Icon, Painter, Weight, rect};
use super::theme::{THEMES, Theme, mix};
use super::{Hit, LayoutMode, Section};

pub struct Shortcut {
    pub name: &'static str,
    pub url: &'static str,
    pub color: u32,
    pub glyph: &'static str,
    pub wide: bool,
}

pub const SHORTCUTS: [Shortcut; 8] = [
    Shortcut { name: "Wikipédia", url: "https://pt.wikipedia.org/wiki/Wikip%C3%A9dia:P%C3%A1gina_principal", color: 0xeaecf0, glyph: "W", wide: true },
    Shortcut { name: "GitHub", url: "https://github.com/explore", color: 0x8957e5, glyph: "G", wide: false },
    Shortcut { name: "DuckDuckGo", url: "https://html.duckduckgo.com/html/", color: 0xde5833, glyph: "D", wide: true },
    Shortcut { name: "Gmail", url: "https://mail.google.com/", color: 0xea4335, glyph: "M", wide: false },
    Shortcut { name: "Hacker News", url: "https://news.ycombinator.com/", color: 0xff6600, glyph: "Y", wide: false },
    Shortcut { name: "YouTube", url: "https://www.youtube.com/", color: 0xff0033, glyph: "▶", wide: true },
    Shortcut { name: "g1", url: "https://g1.globo.com/", color: 0xc4170c, glyph: "g1", wide: false },
    Shortcut { name: "Rust", url: "https://www.rust-lang.org/pt-BR", color: 0xb7410e, glyph: "R", wide: true },
];

fn rgb(v: u32) -> Color {
    Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

const WEEKDAYS: [&str; 7] = [
    "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado", "domingo",
];
const MONTHS: [&str; 12] = [
    "janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro",
    "novembro", "dezembro",
];

type Hits = Vec<(Rect, Hit)>;

/// Ícone de atalho: quadrado arredondado com a cor da marca e a inicial.
pub fn brand_icon(p: &mut Painter, r: Rect, s: &Shortcut) {
    let bg = rgb(s.color);
    p.rrect(r, r.w * 0.25, bg);
    let light_bg = (bg.r as u32 + bg.g as u32 + bg.b as u32) > 600;
    let fg = if light_bg { Color::rgb(0x20, 0x21, 0x22) } else { Color::WHITE };
    if s.glyph == "▶" {
        p.icon(r.x + r.w / 2.0, r.y + r.h / 2.0, Icon::Play, r.w * 0.6, fg);
        return;
    }
    let size = r.h * 0.45;
    let w = p.measure(s.glyph, size, Weight::Bold);
    p.text(r.x + (r.w - w) / 2.0, Painter::baseline(r.y + r.h / 2.0, size), s.glyph, size, fg, Weight::Bold);
}

