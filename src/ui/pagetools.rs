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

/// Estado das ferramentas de uma aba.
#[derive(Default)]
pub struct Tools {
    pub runs: Vec<Run>,
    /// Para qual quadro os trechos foram extraídos (endereço da lista de itens).
    runs_of: usize,
    pub find_open: bool,
    pub find: TextField,
    /// Ocorrências: (trecho, início, fim).
    pub matches: Vec<(usize, usize, usize)>,
    pub current: usize,
    pub sel: Option<(Caret, Caret)>,
    pub selecting: bool,
    pub press: Option<(f32, f32)>,
}

impl Tools {
    /// Atualiza os trechos se o quadro mudou.
    pub fn sync(&mut self, items: &[DisplayItem]) -> bool {
        let key = items.as_ptr() as usize ^ items.len();
        if key == self.runs_of {
            return false;
        }
        self.runs = runs_of(items);
        self.runs_of = key;
        self.sel = None;
        true
    }

    /// Recalcula as ocorrências da busca (sem diferenciar maiúsculas).
    pub fn search(&mut self) {
        self.matches.clear();
        let q = self.find.text.to_lowercase();
        if q.is_empty() {
            return;
        }
        for (ri, r) in self.runs.iter().enumerate() {
            let lower = r.text.to_lowercase();
            // só dá para mapear de volta se o minúsculo tem o mesmo tamanho
            if lower.len() != r.text.len() {
                continue;
            }
            let mut from = 0;
            while let Some(p) = lower[from..].find(&q) {
                let a = from + p;
                self.matches.push((ri, a, a + q.len()));
                from = a + q.len().max(1);
            }
        }
        if self.current >= self.matches.len() {
            self.current = 0;
        }
    }

    /// Ponto do documento → posição no texto mais próxima.
    pub fn caret_at(&self, fonts: &Fonts, x: f32, y: f32) -> Option<Caret> {
        let mut best: Option<(f32, Caret)> = None;
        for (i, r) in self.runs.iter().enumerate() {
            let (top, bottom) = (r.top(), r.bottom());
            let dy = if y < top { top - y } else if y > bottom { y - bottom } else { 0.0 };
            let w = fonts.measure(r.font, r.size, &r.text);
            let dx = if x < r.x { r.x - x } else if x > r.x + w { x - r.x - w } else { 0.0 };
            // linha certa pesa mais que a distância na horizontal
            let d = dy * 1000.0 + dx;
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, (i, r.offset_at(fonts, x))));
            }
        }
        best.map(|(_, c)| c)
    }

    /// Seleção em ordem (início, fim).
    pub fn ordered(&self) -> Option<(Caret, Caret)> {
        let (a, b) = self.sel?;
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        (a != b).then_some((a, b))
    }

    /// Trechos selecionados: (trecho, início, fim).
    pub fn selected_spans(&self) -> Vec<(usize, usize, usize)> {
        let Some(((ra, oa), (rb, ob))) = self.ordered() else { return Vec::new() };
        (ra..=rb.min(self.runs.len().saturating_sub(1)))
            .map(|i| {
                let len = self.runs[i].text.len();
                let a = if i == ra { oa.min(len) } else { 0 };
                let b = if i == rb { ob.min(len) } else { len };
                (i, a, b)
            })
            .filter(|(_, a, b)| b > a)
            .collect()
    }

