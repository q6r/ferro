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

