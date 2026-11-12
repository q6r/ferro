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

    /// Texto selecionado (quebras de linha onde a linha muda).
    pub fn selected_text(&self, fonts: &Fonts) -> String {
        let mut out = String::new();
        let mut prev: Option<(f32, f32, f32)> = None; // (baseline, fim x, tamanho)
        for (i, a, b) in self.selected_spans() {
            let r = &self.runs[i];
            let piece = &r.text[a..b];
            if let Some((base, end, size)) = prev {
                if (r.baseline - base).abs() > size * 0.5 {
                    out.push('\n');
                } else if r.x_at(fonts, a) - end > 1.5 && !out.ends_with(' ') && !piece.starts_with(' ') {
                    out.push(' ');
                }
            }
            out.push_str(piece);
            prev = Some((r.baseline, r.x_at(fonts, b), r.size));
        }
        out
    }

    pub fn select_all(&mut self) {
        if let Some(last) = self.runs.len().checked_sub(1) {
            self.sel = Some(((0, 0), (last, self.runs[last].text.len())));
        }
    }
}

/// Aviso rápido no canto da página.
pub struct Toast {
    pub text: String,
    pub since: Instant,
}

impl App {
    /// Destaques da busca e da seleção por cima da página.
    pub(super) fn draw_page_marks(&self, p: &mut Painter, area: Rect, scroll: f32) {
        let tools = &self.tools;
        let fonts = p.fonts;
        let z = self.zoom;
        let to_screen = |r: Rect| rect(area.x + r.x * z, area.y + (r.y - scroll) * z, r.w * z, r.h * z);
        let visible = |r: &Rect| r.y + r.h >= area.y && r.y <= area.y + area.h;
        if tools.find_open {
            for (k, &(ri, a, b)) in tools.matches.iter().enumerate() {
                let Some(run) = tools.runs.get(ri) else { continue };
                let r = to_screen(run.span(fonts, a, b));
                if !visible(&r) {
                    continue;
                }
                let c = if k == tools.current {
                    Color { r: 255, g: 150, b: 50, a: 150 }
                } else {
                    Color { r: 255, g: 230, b: 0, a: 110 }
                };
                p.rect(r, c);
            }
        }
        for (ri, a, b) in tools.selected_spans() {
            let r = to_screen(tools.runs[ri].span(fonts, a, b));
            if visible(&r) {
                p.rect(r, Color { r: 60, g: 120, b: 255, a: 90 });
            }
        }
    }

    /// Barra de busca na página (canto superior direito da página).
    pub(super) fn draw_find_bar(&self, p: &mut Painter, t: &Theme, g: &Geo, hits: &mut Hits) {
        if !self.tools.find_open {
            return;
        }
        let c = g.content;
        let w = 340.0f32.min(c.w - 24.0);
        let r = rect(c.x + c.w - w - 16.0, c.y + 10.0, w, 40.0);
        p.shadow(r, 12.0, 30);
        p.rrect(r, 12.0, t.popup);
        p.stroke(r, 12.0, 1.0, t.border);
        p.icon(r.x + 20.0, r.y + 20.0, Icon::Search, 16.0, t.muted);
        let count = if self.tools.find.text.is_empty() {
            String::new()
        } else if self.tools.matches.is_empty() {
            "0/0".to_string()
        } else {
            format!("{}/{}", self.tools.current + 1, self.tools.matches.len())
        };
        let cw = p.measure(&count, 12.0, Weight::Regular);
        let tx = r.x + 36.0;
        let field_w = r.w - 36.0 - cw - 52.0;
        let base = Painter::baseline(r.y + 20.0, 14.0);
        let text = &self.tools.find.text;
        let shown = p.ellipsize(text, 14.0, Weight::Regular, field_w);
        if text.is_empty() {
            p.text(tx, base, "Buscar na página", 14.0, t.muted, Weight::Regular);
        } else {
            p.text(tx, base, &shown, 14.0, t.text, Weight::Regular);
        }
        // cursor
        let caret_x = tx + p.measure(&shown, 14.0, Weight::Regular).min(field_w);
        p.rect(rect(caret_x + 1.0, r.y + 11.0, 1.5, 18.0), t.text);
        let muted = if !text.is_empty() && self.tools.matches.is_empty() {
            Color::rgb(0xe0, 0x45, 0x45)
        } else {
            t.muted
        };
        p.text(r.x + r.w - 40.0 - cw, base - 1.0, &count, 12.0, muted, Weight::Regular);
        let close = rect(r.x + r.w - 34.0, r.y + 6.0, 28.0, 28.0);
        if self.hover == Some(Hit::FindClose) {
            p.rrect(close, 8.0, t.row_selected());
        }
        p.icon(close.x + 14.0, close.y + 14.0, Icon::Close, 14.0, t.text);
        hits.push((r, Hit::FindBar));
        hits.push((close, Hit::FindClose));
    }

    /// Lista aberta de um `<select>`, logo abaixo (ou acima) dele.
    pub(super) fn draw_select(&self, p: &mut Painter, t: &Theme, g: &Geo, hits: &mut Hits) {
        let Some((_, popup, hl, first)) = &self.select else { return };
        const ROW: f32 = 28.0;
        const ROWS: usize = 10;
        let c = g.content;
        let z = self.zoom;
        let scroll = self.effective_scroll(self.active);
        let sx = c.x + popup.rect.x * z;
        let sy = c.y + (popup.rect.y - scroll) * z;
        let sh = popup.rect.h * z;
        let widest = popup.labels.iter().map(|l| p.measure(l, 14.0, Weight::Regular)).fold(0.0, f32::max);
        let w = (popup.rect.w * z).max(widest + 36.0).min(c.w - 8.0);
        let n = popup.labels.len().min(ROWS);
        let h = n as f32 * ROW + 8.0;
        // abre para baixo; sem espaço, para cima
        let y = if sy + sh + h <= c.y + c.h { sy + sh + 2.0 } else { (sy - h - 2.0).max(c.y) };
        let x = sx.clamp(c.x, c.x + c.w - w);
        let r = rect(x, y, w, h);
        p.shadow(r, 10.0, 34);
        p.rrect(r, 10.0, t.popup);
        p.stroke(r, 10.0, 1.0, t.border);
        hits.push((r, Hit::SelectList));
        for k in 0..n {
            let i = *first + k;
            let Some(label) = popup.labels.get(i) else { break };
            let row = rect(x + 4.0, y + 4.0 + k as f32 * ROW, w - 8.0, ROW);
            let disabled = popup.disabled.get(i).copied().unwrap_or(false);
            let hover = self.hover == Some(Hit::SelectOption(i));
            if i == *hl || hover {
                p.rrect(row, 7.0, if hover && i != *hl { t.row_selected() } else { t.accent });
            }
            let color = if i == *hl && !hover { t.on_accent } else if disabled { t.muted } else { t.text };
            let text = p.ellipsize(label, 14.0, Weight::Regular, row.w - 20.0);
            p.text(row.x + 10.0, Painter::baseline(row.y + ROW / 2.0, 14.0), &text, 14.0, color, Weight::Regular);
            if i as i32 == popup.selected {
                p.rect(rect(row.x + 2.0, row.y + 8.0, 3.0, ROW - 16.0), if i == *hl { t.on_accent } else { t.accent });
            }
            if !disabled {
                hits.push((row, Hit::SelectOption(i)));
            }
        }
    }

    /// Menu de contexto (botão direito).
    pub(super) fn draw_menu(&self, p: &mut Painter, t: &Theme, hits: &mut Hits) {
        let Some(((mx, my), items)) = &self.menu else { return };
        const ROW: f32 = 30.0;
        const SEP: f32 = 9.0;
        let (ww, wh) = self.logical_size();
        let widest = items.iter().map(|(l, _)| p.measure(l, 13.5, Weight::Regular)).fold(0.0, f32::max);
        let w = (widest + 44.0).max(190.0);
        let h: f32 = items.iter().map(|(_, a)| if a.is_some() { ROW } else { SEP }).sum::<f32>() + 10.0;
        let x = mx.min(ww - w - 6.0).max(6.0);
        let y = if my + h > wh - 6.0 { (my - h).max(6.0) } else { *my };
        let r = rect(x, y, w, h);
        p.shadow(r, 12.0, 36);
        p.rrect(r, 12.0, t.popup);
        p.stroke(r, 12.0, 1.0, t.border);
        hits.push((r, Hit::Menu));
        let mut cy = y + 5.0;
        for (i, (label, action)) in items.iter().enumerate() {
            if action.is_none() {
                p.rect(rect(x + 12.0, cy + SEP / 2.0, w - 24.0, 1.0), t.border);
                cy += SEP;
                continue;
            }
            let row = rect(x + 5.0, cy, w - 10.0, ROW);
            if self.hover == Some(Hit::MenuItem(i)) {
                p.rrect(row, 8.0, t.row_selected());
            }
            p.text(row.x + 14.0, Painter::baseline(row.y + ROW / 2.0, 13.5), label, 13.5, t.text, Weight::Regular);
            hits.push((row, Hit::MenuItem(i)));
            cy += ROW;
        }
    }

