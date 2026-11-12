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

