//! Desenho da interface: trilho de ícones, lista de abas, abas em cápsulas,
//! barra com a cápsula de endereço, o painel flutuante da página e a
//! paleta de busca de vidro fosco.

use crate::layout::Rect;
use crate::paint::Transform;
use crate::style::Color;

use super::pages::{self, SHORTCUTS};
use super::painter::{Icon, Painter, Weight, rect};
use super::theme::{Theme, mix, with_alpha};
use super::{App, Geo, Hit, Hits, Kind, display_url};

/// Tamanho do texto digitado na paleta.
pub const PALETTE_FONT: f32 = 20.0;

fn palette_box(w: f32, h: f32) -> Rect {
    let pw = (w - 64.0).min(720.0);
    rect((w - pw) / 2.0, (h * 0.14).max(40.0), pw, 64.0)
}

/// Onde começa o texto digitado na paleta (para posicionar o cursor).
pub fn palette_text_x(w: f32) -> f32 {
    palette_box(w, 0.0).x + 64.0
}

/// Fundo de um item sob o mouse, entre o fundo da janela e o selecionado.
fn hover_bg(t: &Theme) -> Color {
    mix(t.frame, t.row_selected(), 0.55)
}

impl App {
    // ------------------------------------------------------------ página

    /// Painel flutuante com a página. Devolve a geometria da barra de rolagem.
    pub(super) fn draw_content(&self, p: &mut Painter, t: &Theme, g: &Geo, hits: &mut Hits) -> Option<(Rect, f32)> {
        let area = g.content;
        if self.fullscreen.is_some() {
            p.clip = Some(area);
            let scrollbar = self.draw_page(p, t, area, hits);
            p.clip = None;
            return scrollbar;
        }
        let radius = 14.0;
        if !t.dark {
            p.shadow(area, radius, 26);
        }
        p.clip = Some(area);
        let scrollbar = self.draw_page(p, t, area, hits);
        p.clip = None;
        p.round_corners(area, radius, t.frame);
        // as configurações já têm a própria moldura (kit), sem borda dupla
        if !matches!(self.tab().kind(), Kind::Settings(_)) {
            p.stroke(area, radius, 1.0, t.border);
        }
        scrollbar
    }

    fn draw_page(&self, p: &mut Painter, t: &Theme, area: Rect, hits: &mut Hits) -> Option<(Rect, f32)> {
        let tab = self.tab();
        match tab.kind() {
            Kind::NewTab => {
                pages::new_tab(p, t, area, hits, self.hover, &jiff::Zoned::now(), self.palette_open);
                None
            }
            Kind::Settings(sec) => {
                pages::settings(p, t, self.theme, self.layout, area, sec, hits, self.hover);
                None
            }
            Kind::Web => {
                if let Some(err) = &tab.error {
                    pages::error(p, t, area, tab.url().as_str(), err, hits, self.hover);
                    return None;
                }
                let Some(frame) = &tab.frame else {
                    p.rect(area, t.ntp_bg);
                    return None;
                };
                p.rect(area, frame.background);
                let scroll = self.effective_scroll(self.active);
                let s = p.scale;
                let sc = s * self.zoom;
                let tr = Transform {
                    scale: sc,
                    dx: area.x * s,
                    dy: area.y * s - scroll * sc,
                };
                let clip = Rect {
                    x: area.x * s,
                    y: area.y * s,
                    w: area.w * s,
                    h: area.h * s,
                };
                p.canvas.videos = frame.videos.clone();
                p.canvas.paused = frame.paused.clone();
                p.canvas.fixed_shift = scroll * sc;
                p.canvas.paint_list(&frame.items, p.fonts, tr, Some(clip));
                p.canvas.fixed_shift = 0.0;
                self.draw_page_marks(p, area, scroll);
                // na tela cheia não há barra de rolagem
                if self.fullscreen.is_some() {
                    return None;
                }

                // barra de rolagem
                let view_h = area.h / self.zoom;
                if frame.height <= view_h + 1.0 {
                    return None;
                }
                let max = frame.height - view_h;
                let track = rect(area.x + area.w - 13.0, area.y + 8.0, 10.0, area.h - 16.0);
                let th = (track.h * view_h / frame.height).max(36.0);
                let ty = track.y + (track.h - th) * (scroll / max);
                let active =
                    matches!(self.hover, Some(Hit::ScrollThumb | Hit::ScrollTrack)) || self.drag_scroll.is_some();
                let thumb = if active {
                    rect(track.x + 1.0, ty, 8.0, th)
                } else {
                    rect(track.x + 3.5, ty, 5.0, th)
                };
                let gray = Color {
                    r: 128,
                    g: 128,
                    b: 128,
                    a: if active { 210 } else { 140 },
                };
                p.rrect(thumb, thumb.w / 2.0, gray);
                hits.push((track, Hit::ScrollTrack));
                hits.push((rect(track.x, ty, track.w, th), Hit::ScrollThumb));
                Some((track, th))
            }
        }
    }

    // ----------------------------------------------------- moldura

    pub(super) fn draw_chrome(&self, p: &mut Painter, t: &Theme, g: &Geo, hits: &mut Hits) {
        if let Some(r) = g.rail {
            self.draw_rail(p, t, r, g.panel.is_some(), hits);
        }
        if let Some(r) = g.panel {
            self.draw_tab_panel(p, t, r, hits);
        }
        if let Some(r) = g.tabs_row {
            self.draw_tab_chips(p, t, r, hits);
        }
        self.draw_bar(p, t, g, hits);
    }

    fn logo(&self, p: &mut Painter, t: &Theme, x: f32, y: f32, size: f32, on_rail: bool) {
        if on_rail {
            let bg = if t.dark { t.rail_icon_active() } else { Color::WHITE };
            let spark = if t.dark { t.rail() } else { t.accent };
            p.logo(x, y, size, bg, t.rail(), spark);
        } else {
            p.logo(x, y, size, t.text, t.frame, if t.dark { t.frame } else { t.accent });
        }
    }

    /// Trilho de ícones (kit 4).
    fn draw_rail(&self, p: &mut Painter, t: &Theme, r: Rect, panel_open: bool, hits: &mut Hits) {
        p.rrect(r, 16.0, t.rail());
        self.logo(p, t, r.x + 11.0, r.y + 12.0, 36.0, true);
        let kind = self.tab().kind();
        let top = [
            (Hit::NewTab, Icon::Plus, false),
            (Hit::Search, Icon::Search, self.palette_open),
            (Hit::Home, Icon::Home, kind == Kind::NewTab),
        ];
        let mut y = r.y + 76.0;
        for (hit, icon, active) in top {
            self.rail_button(p, t, rect(r.x + 7.0, y, 44.0, 44.0), hit, icon, active, hits);
            y += 52.0;
        }
        let bottom = [
            (Hit::Settings, Icon::Settings, matches!(kind, Kind::Settings(_))),
            (Hit::TogglePanel, Icon::Panel, panel_open),
        ];
        let mut y = r.y + r.h - 54.0;
        for (hit, icon, active) in bottom {
            self.rail_button(p, t, rect(r.x + 7.0, y, 44.0, 44.0), hit, icon, active, hits);
            y -= 52.0;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rail_button(&self, p: &mut Painter, t: &Theme, b: Rect, hit: Hit, icon: Icon, active: bool, hits: &mut Hits) {
        let hovered = self.hover == Some(hit);
        if active {
            p.rrect(b, 12.0, t.rail_active());
        } else if hovered {
            p.rrect(b, 12.0, with_alpha(t.rail_active(), 140));
        }
        let color = if active || hovered { t.rail_icon_active() } else { t.rail_icon() };
        p.icon(b.x + 22.0, b.y + 22.0, icon, 22.0, color);
        hits.push((b, hit));
    }

    /// Lista de abas ao lado do trilho (kit 4: "Opportunities").
    fn draw_tab_panel(&self, p: &mut Painter, t: &Theme, r: Rect, hits: &mut Hits) {
        let x = r.x + 6.0;
        let w = r.w - 12.0;
        let limit = r.y + r.h - 8.0;
        p.text(x + 12.0, r.y + 36.0, "Abas", 19.0, t.text, Weight::Semibold);
        let n = self.tabs.len().to_string();
        let nw = p.measure(&n, 16.0, Weight::Regular);
        p.text(x + w - 14.0 - nw, r.y + 36.0, &n, 16.0, t.muted, Weight::Regular);

        let mut y = r.y + 56.0;
        for i in 0..self.tabs.len() {
            if y + 40.0 > limit {
                break;
            }
            self.draw_tab_row(p, t, i, rect(x, y, w, 40.0), hits);
            y += 44.0;
        }
        let row = rect(x, y, w, 40.0);
        if self.hover == Some(Hit::NewTab) {
            p.rrect(row, 12.0, hover_bg(t));
        }
        p.icon(row.x + 22.0, row.y + 20.0, Icon::Plus, 17.0, t.muted);
        p.text(row.x + 44.0, Painter::baseline(row.y + 20.0, 14.0), "Nova aba", 14.0, t.muted, Weight::Regular);
        hits.push((row, Hit::NewTab));
        y += 56.0;

        if y + 70.0 < limit {
            p.rect(rect(x + 12.0, y, w - 24.0, 1.0), t.border);
            y += 30.0;
            p.text(x + 12.0, y, "Atalhos", 13.0, t.muted, Weight::Regular);
            y += 12.0;
            for (i, s) in SHORTCUTS.iter().enumerate() {
                if y + 38.0 > limit {
                    break;
                }
                let row = rect(x, y, w, 38.0);
                if self.hover == Some(Hit::Shortcut(i)) {
                    p.rrect(row, 12.0, hover_bg(t));
                }
                pages::brand_icon(p, rect(row.x + 12.0, row.y + 8.0, 22.0, 22.0), s);
                p.text(row.x + 46.0, Painter::baseline(row.y + 19.0, 14.0), s.name, 14.0, t.text, Weight::Regular);
                hits.push((row, Hit::Shortcut(i)));
                y += 40.0;
            }
        }
    }

    fn draw_tab_row(&self, p: &mut Painter, t: &Theme, i: usize, row: Rect, hits: &mut Hits) {
        let tab = &self.tabs[i];
        let active = i == self.active;
        let hovered = matches!(self.hover, Some(Hit::Tab(j) | Hit::CloseTab(j)) if j == i);
        if active {
            if !t.dark {
                p.shadow(row, 12.0, 14);
            }
            p.rrect(row, 12.0, t.row_selected());
            p.stroke(row, 12.0, 1.0, t.border);
        } else if hovered {
            p.rrect(row, 12.0, hover_bg(t));
        }
        hits.push((row, Hit::Tab(i)));
        let ic = if tab.loading {
            t.accent
        } else if active {
            t.text
        } else {
            t.muted
        };
        p.icon(row.x + 22.0, row.y + 20.0, tab.icon(), 17.0, ic);
        let show_close = active || hovered;
        let right = row.x + row.w - if show_close { 36.0 } else { 12.0 };
        let weight = if active { Weight::Semibold } else { Weight::Regular };
        let title = p.ellipsize(&tab.title(), 14.0, weight, right - (row.x + 42.0));
        p.text(row.x + 42.0, Painter::baseline(row.y + 20.0, 14.0), &title, 14.0, t.text, weight);
        if show_close {
            let cb = rect(row.x + row.w - 30.0, row.y + 10.0, 20.0, 20.0);
            if self.hover == Some(Hit::CloseTab(i)) {
                p.rrect(cb, 10.0, t.field_hover);
            }
            p.icon(cb.x + 10.0, cb.y + 10.0, Icon::Close, 13.0, t.muted);
            hits.push((cb, Hit::CloseTab(i)));
        }
    }

    /// Abas em pílulas no topo (layout "Cápsulas").
    fn draw_tab_chips(&self, p: &mut Painter, t: &Theme, r: Rect, hits: &mut Hits) {
        self.logo(p, t, r.x + 12.0, r.y + 2.0, 32.0, false);
        let start = r.x + 56.0;
        let avail = r.w - start - 100.0;
        let cw = (avail / self.tabs.len().max(1) as f32).clamp(72.0, 220.0);
        for i in 0..self.tabs.len() {
            let chip = rect(start + i as f32 * cw, r.y + 2.0, cw - 8.0, 32.0);
            let tab = &self.tabs[i];
            let active = i == self.active;
            let hovered = matches!(self.hover, Some(Hit::Tab(j) | Hit::CloseTab(j)) if j == i);
            if active {
                if !t.dark {
                    p.shadow(chip, 16.0, 14);
                }
                p.rrect(chip, 16.0, t.row_selected());
                p.stroke(chip, 16.0, 1.0, t.border);
            } else if hovered {
                p.rrect(chip, 16.0, hover_bg(t));
            }
            hits.push((chip, Hit::Tab(i)));
            let ic = if tab.loading {
                t.accent
            } else if active {
                t.text
            } else {
                t.muted
            };
            p.icon(chip.x + 18.0, chip.y + 16.0, tab.icon(), 16.0, ic);
            let show_close = (active || hovered) && chip.w > 90.0;
            let right = chip.x + chip.w - if show_close { 32.0 } else { 12.0 };
            if right - (chip.x + 34.0) > 14.0 {
                let title = p.ellipsize(&tab.title(), 13.0, Weight::Regular, right - (chip.x + 34.0));
                let c = if active { t.text } else { t.muted };
                p.text(chip.x + 34.0, Painter::baseline(chip.y + 16.0, 13.0), &title, 13.0, c, Weight::Regular);
            }
            if show_close {
                let cb = rect(chip.x + chip.w - 27.0, chip.y + 6.0, 20.0, 20.0);
                if self.hover == Some(Hit::CloseTab(i)) {
                    p.rrect(cb, 10.0, t.field_hover);
                }
                p.icon(cb.x + 10.0, cb.y + 10.0, Icon::Close, 12.0, t.muted);
                hits.push((cb, Hit::CloseTab(i)));
            }
        }
        let nb = rect(start + self.tabs.len() as f32 * cw, r.y + 2.0, 32.0, 32.0);
        if self.hover == Some(Hit::NewTab) {
            p.rrect(nb, 16.0, hover_bg(t));
        }
        p.icon(nb.x + 16.0, nb.y + 16.0, Icon::Plus, 18.0, t.text);
        hits.push((nb, Hit::NewTab));

        let sb = rect(r.x + r.w - 44.0, r.y + 2.0, 32.0, 32.0);
        if self.hover == Some(Hit::Settings) {
            p.rrect(sb, 16.0, hover_bg(t));
        }
        p.icon(sb.x + 16.0, sb.y + 16.0, Icon::Settings, 19.0, t.text);
        hits.push((sb, Hit::Settings));
    }

    /// Navegação e a cápsula de endereço (abre a paleta ao clicar).
    fn draw_bar(&self, p: &mut Painter, t: &Theme, g: &Geo, hits: &mut Hits) {
        let b = g.bar;
        let tab = self.tab();
        let y = b.y + 6.0;
        let buttons = [
            (Hit::Back, Icon::Back, tab.index > 0),
            (Hit::Forward, Icon::Forward, tab.index + 1 < tab.history.len()),
            (Hit::Reload, if tab.loading { Icon::Close } else { Icon::Reload }, tab.kind() == Kind::Web),
        ];
        for (k, (hit, icon, enabled)) in buttons.into_iter().enumerate() {
            let r = rect(b.x + 2.0 + k as f32 * 38.0, y, 32.0, 32.0);
            if enabled && self.hover == Some(hit) {
                p.rrect(r, 16.0, hover_bg(t));
            }
            let c = if enabled { t.text } else { with_alpha(t.muted, 90) };
            p.icon(r.x + 16.0, r.y + 16.0, icon, 19.0, c);
            if enabled {
                hits.push((r, hit));
            }
        }

        let c = g.capsule;
        let hov = self.hover == Some(Hit::Capsule);
        p.rrect(c, 18.0, if hov { t.card_hover } else { t.card });
        p.stroke(c, 18.0, 1.0, t.border);
        hits.push((c, Hit::Capsule));

        let zoom = (self.zoom != 1.0).then(|| format!("{:.0}%", self.zoom * 100.0));
        let zoom_w = zoom.as_ref().map_or(0.0, |z| p.measure(z, 12.0, Weight::Semibold) + 18.0);
        let clip = rect(c.x + 12.0, c.y, c.w - 24.0 - zoom_w, c.h);
        let base = Painter::baseline(c.y + 18.0, 14.0);
        if tab.kind() == Kind::NewTab {
            let label = "Pesquise ou digite um endereço";
            let lw = p.measure(label, 14.0, Weight::Regular) + 24.0;
            let x0 = c.x + (c.w - lw) / 2.0;
            p.icon(x0 + 8.0, c.y + 18.0, Icon::Search, 16.0, t.muted);
            p.text_clipped(x0 + 24.0, base, label, 14.0, t.muted, Weight::Regular, Some(clip));
        } else {
            let (domain, rest) = display_url(tab.url());
            let icon = match tab.url().scheme() {
                "https" => Icon::Lock,
                "http" => Icon::Unlocked,
                "ferro" => Icon::Settings,
                _ => Icon::File,
            };
            let dw = p.measure(&domain, 14.0, Weight::Semibold);
            let rw = p.measure(&rest, 14.0, Weight::Regular);
            let total = 24.0 + dw + rw;
            let x0 = if total <= clip.w { c.x + (c.w - total) / 2.0 } else { c.x + 14.0 };
            p.icon(x0 + 8.0, c.y + 18.0, icon, 15.0, t.muted);
            p.text_clipped(x0 + 24.0, base, &domain, 14.0, t.text, Weight::Semibold, Some(clip));
            p.text_clipped(x0 + 24.0 + dw, base, &rest, 14.0, t.muted, Weight::Regular, Some(clip));
        }
        if let Some(z) = zoom {
            let zr = rect(c.x + c.w - zoom_w - 4.0, c.y + 7.0, zoom_w - 4.0, 22.0);
            p.rrect(zr, 11.0, t.field_hover);
            p.text(zr.x + 7.0, Painter::baseline(zr.y + 11.0, 12.0), &z, 12.0, t.text, Weight::Semibold);
        }
        if tab.loading {
            let bar = rect(c.x + 18.0, c.y + c.h - 4.0, (c.w - 36.0) * tab.progress, 2.5);
            p.rrect(bar, 1.25, t.accent);
        }
    }

