//! Ferramentas sobre a página pintada: buscar na página (Ctrl+F), selecionar
//! e copiar texto, e os avisos rápidos (downloads).
//!
//! Tudo trabalha sobre os trechos de texto do quadro (`DisplayItem::Text`),
//! em coordenadas do documento.

use std::time::{Duration, Instant};

use crate::fonts::{FontId, Fonts};
use crate::layout::Rect;
use crate::paint::DisplayItem;
use crate::style::Color;

use super::painter::{Icon, Painter, Weight, rect};
use super::textfield::TextField;
use super::theme::Theme;
use super::{App, Geo, Hit, Hits};

/// Um trecho de texto pintado.
pub struct Run {
    pub x: f32,
    pub baseline: f32,
    pub size: f32,
    pub font: FontId,
    pub text: String,
}

impl Run {
    fn top(&self) -> f32 {
        self.baseline - self.size * 0.95
    }

    fn bottom(&self) -> f32 {
        self.baseline + self.size * 0.3
    }

    /// Posição x (documento) do byte `i` do texto.
    fn x_at(&self, fonts: &Fonts, i: usize) -> f32 {
        self.x + fonts.measure(self.font, self.size, &self.text[..i])
    }

    /// Retângulo dos bytes `a..b`.
    fn span(&self, fonts: &Fonts, a: usize, b: usize) -> Rect {
        let x0 = self.x_at(fonts, a);
        let x1 = self.x_at(fonts, b);
        Rect { x: x0, y: self.top(), w: (x1 - x0).max(1.0), h: self.bottom() - self.top() }
    }

    /// Byte mais perto do x dado.
    fn offset_at(&self, fonts: &Fonts, x: f32) -> usize {
        let mut prev = self.x;
        for (i, c) in self.text.char_indices() {
            let next = prev + fonts.advance(self.font, c, self.size);
            if x < (prev + next) / 2.0 {
                return i;
            }
            prev = next;
        }
        self.text.len()
    }
}

pub fn runs_of(items: &[DisplayItem]) -> Vec<Run> {
    items
        .iter()
        .filter_map(|it| match it {
            DisplayItem::Text { x, baseline, text, font, size, color } if color.a > 0 && !text.trim().is_empty() => Some(Run {
                x: *x,
                baseline: *baseline,
                size: *size,
                font: *font,
                text: text.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Posição no texto: (trecho, byte).
pub type Caret = (usize, usize);

