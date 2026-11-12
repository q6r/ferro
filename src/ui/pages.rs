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

/// Nova aba: relógio, data, busca e atalhos (kit `uikits/...-0.png`).
#[allow(clippy::too_many_arguments)]
pub fn new_tab(
    p: &mut Painter,
    t: &Theme,
    area: Rect,
    hits: &mut Hits,
    hover: Option<Hit>,
    now: &jiff::Zoned,
    search_focused: bool,
) {
    p.rect(area, t.ntp_bg);
    // grade de pontos
    let step = 22.0;
    let mut y = area.y + step / 2.0;
    while y < area.y + area.h {
        let mut x = area.x + step / 2.0;
        while x < area.x + area.w {
            p.rect(rect(x, y, 1.6, 1.6), t.ntp_dot);
            x += step;
        }
        y += step;
    }

    let cx = area.x + area.w / 2.0;
    let compact = area.h < 640.0;
    let clock_size = if compact { 76.0 } else { 104.0 };
    let clock = format!("{:02}:{:02}", now.hour(), now.minute());
    let clock_w = p.measure(&clock, clock_size, Weight::Semibold);
    let clock_base = area.y + if compact { 110.0 } else { (area.h * 0.24).max(150.0) };
    p.text(cx - clock_w / 2.0, clock_base, &clock, clock_size, t.text, Weight::Semibold);

    let wd = now.weekday().to_monday_zero_offset() as usize;
    let date = format!("{}, {} de {}", WEEKDAYS[wd], now.day(), MONTHS[(now.month() - 1) as usize]);
    let date_w = p.measure(&date, 22.0, Weight::Regular);
    let date_base = clock_base + 42.0;
    p.text(cx - date_w / 2.0, date_base, &date, 22.0, t.muted, Weight::Regular);

    // caixa de busca (abre a paleta)
    let bw = (area.w - 64.0).min(680.0);
    let sb = rect(cx - bw / 2.0, date_base + 34.0, bw, 56.0);
    let hov = hover == Some(Hit::FakeBox);
    p.rrect(sb, 28.0, if hov { t.card_hover } else { t.card });
    p.stroke(sb, 28.0, 1.0, if search_focused { t.accent } else { t.border });
    p.icon(sb.x + 30.0, sb.y + 28.0, Icon::Search, 22.0, t.muted);
    p.text(
        sb.x + 56.0,
        Painter::baseline(sb.y + 28.0, 16.0),
        "Pesquise na web ou digite um endereço",
        16.0,
        t.muted,
        Weight::Regular,
    );
    hits.push((sb, Hit::FakeBox));

    // atalhos em duas fileiras, alternando cartões largos e compactos
    let (wide_w, tile, gap): (f32, f32, f32) = (190.0, 60.0, 14.0);
    let slot = |p: &Painter, s: &Shortcut| {
        if s.wide { wide_w } else { tile.max(p.measure(s.name, 14.0, Weight::Semibold) + 6.0) }
    };
    let mut y = sb.y + sb.h + 30.0;
    for row in SHORTCUTS.chunks(4) {
        let total: f32 = row.iter().map(|s| slot(p, s)).sum::<f32>() + gap * 3.0;
        let mut x = cx - total / 2.0;
        for s in row {
            let idx = SHORTCUTS.iter().position(|o| o.name == s.name).unwrap();
            let hov = hover == Some(Hit::Shortcut(idx));
            if s.wide {
                let r = rect(x, y, wide_w, 80.0);
                let c = rgb(s.color);
                let light = c.r as u32 + c.g as u32 + c.b as u32 > 600;
                let amount = match (light, hov) {
                    (true, false) => 0.07,
                    (true, true) => 0.13,
                    (false, false) => 0.16,
                    (false, true) => 0.28,
                };
                p.rrect(r, 16.0, mix(t.card, c, amount));
                brand_icon(p, rect(r.x + 16.0, r.y + 20.0, 40.0, 40.0), s);
                p.text(r.x + 68.0, Painter::baseline(r.y + 40.0, 16.0), s.name, 16.0, t.text, Weight::Semibold);
                p.icon(r.x + r.w - 17.0, r.y + 17.0, Icon::ArrowUpRight, 15.0, t.muted);
                hits.push((r, Hit::Shortcut(idx)));
                x += wide_w + gap;
            } else {
                let sw = slot(p, s);
                let r = rect(x + (sw - tile) / 2.0, y, tile, tile);
                p.rrect(r, 16.0, if hov { t.card_hover } else { t.card });
                brand_icon(p, rect(r.x + 14.0, r.y + 14.0, 32.0, 32.0), s);
                let lw = p.measure(s.name, 14.0, Weight::Semibold);
                p.text(x + (sw - lw) / 2.0, r.y + tile + 19.0, s.name, 14.0, t.text, Weight::Semibold);
                hits.push((rect(x, y, sw, tile + 24.0), Hit::Shortcut(idx)));
                x += sw + gap;
            }
        }
        y += 80.0 + 18.0;
    }

    // "Personalizar" leva às configurações
    let label = "Personalizar";
    let lw = p.measure(label, 13.0, Weight::Regular);
    let pw = lw + 52.0;
    let py = (area.y + area.h - 58.0).max(y + 12.0);
    let pr = rect(cx - pw / 2.0, py, pw, 34.0);
    let hov = hover == Some(Hit::Customize);
    p.rrect(pr, 17.0, if hov { t.card_hover } else { t.card });
    p.icon(pr.x + 20.0, pr.y + 17.0, Icon::Swatch, 16.0, t.muted);
    p.text(pr.x + 36.0, Painter::baseline(pr.y + 17.0, 13.0), label, 13.0, t.muted, Weight::Regular);
    hits.push((pr, Hit::Customize));
}

/// Miniatura de navegador nas cores de um tema.
fn theme_preview(p: &mut Painter, card: Rect, th: &Theme) {
    let u = card.w / 100.0;
    p.rrect(card, 12.0, th.frame);
    // trilho lateral e a página flutuante, como no layout padrão
    p.rrect(rect(card.x + 4.0 * u, card.y + 4.0 * u, 8.0 * u, card.h - 8.0 * u), 3.0 * u, th.rail());
    for i in 0..3 {
        let w = [22.0, 16.0, 19.0][i] * u;
        let c = if i == 0 { th.row_selected() } else { th.border };
        p.rrect(rect(card.x + 16.0 * u, card.y + (8.0 + i as f32 * 7.0) * u, w, 4.5 * u), 2.0 * u, c);
    }
    let page = rect(card.x + 42.0 * u, card.y + 6.0 * u, card.w - 46.0 * u, card.h - 10.0 * u);
    p.rrect(page, 4.0 * u, th.toolbar);
    p.rrect(rect(page.x + 4.0 * u, page.y + 4.0 * u, page.w * 0.5, 3.5 * u), 1.75 * u, th.muted);
    for (i, j) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let bw = (page.w - 12.0 * u) / 2.0;
        p.rrect(
            rect(page.x + 4.0 * u + i as f32 * (bw + 4.0 * u), page.y + 12.0 * u + j as f32 * 16.0 * u, bw, 13.0 * u),
            3.0 * u,
            th.card_hover,
        );
    }
}

/// Miniatura de um dos dois layouts, nas cores do tema atual.
fn layout_preview(p: &mut Painter, card: Rect, t: &Theme, mode: LayoutMode) {
    let u = card.w / 100.0;
    p.rrect(card, 12.0, t.frame);
    match mode {
        LayoutMode::Sidebar => {
            p.rrect(rect(card.x + 3.0 * u, card.y + 3.0 * u, 7.0 * u, card.h - 6.0 * u), 2.5 * u, t.rail());
            p.rrect(rect(card.x + 5.0 * u, card.y + 6.0 * u, 3.0 * u, 3.0 * u), 1.0 * u, t.rail_icon_active());
            for i in 0..4 {
                let c = if i == 0 { t.row_selected() } else { t.border };
                p.rrect(rect(card.x + 13.0 * u, card.y + (6.0 + i as f32 * 6.0) * u, 22.0 * u, 4.0 * u), 2.0 * u, c);
            }
            p.rrect(rect(card.x + 52.0 * u, card.y + 4.0 * u, 34.0 * u, 4.0 * u), 2.0 * u, t.field);
            let page = rect(card.x + 39.0 * u, card.y + 11.0 * u, card.w - 42.0 * u, card.h - 14.0 * u);
            p.rrect(page, 3.0 * u, t.toolbar);
        }
        LayoutMode::Capsules => {
            for i in 0..3 {
                let c = if i == 0 { t.row_selected() } else { t.border };
                p.rrect(rect(card.x + (4.0 + i as f32 * 20.0) * u, card.y + 4.0 * u, 17.0 * u, 5.0 * u), 2.5 * u, c);
            }
            p.rrect(rect(card.x + 28.0 * u, card.y + 12.0 * u, 44.0 * u, 5.0 * u), 2.5 * u, t.field);
            let page = rect(card.x + 3.0 * u, card.y + 20.0 * u, card.w - 6.0 * u, card.h - 23.0 * u);
            p.rrect(page, 3.0 * u, t.toolbar);
        }
    }
}

fn ring(p: &mut Painter, card: Rect, color: Color, width: f32) {
    let r = rect(card.x - 3.0, card.y - 3.0, card.w + 6.0, card.h + 6.0);
    p.stroke(r, 15.0, width, color);
}

/// Configurações (kit `uikits/configuracoes.png`).
#[allow(clippy::too_many_arguments)]
pub fn settings(
    p: &mut Painter,
    t: &Theme,
    current: usize,
    layout: LayoutMode,
    area: Rect,
    section: Section,
    hits: &mut Hits,
    hover: Option<Hit>,
) {
    p.rect(area, t.frame);
    let side_x = area.x + 16.0;
    let side_w = 214.0;

    let back = rect(side_x + 4.0, area.y + 18.0, 98.0, 38.0);
    p.rrect(back, 19.0, if hover == Some(Hit::SettingsBack) { t.card_hover } else { t.toolbar });
    p.stroke(back, 19.0, 1.0, t.border);
    p.icon(back.x + 24.0, back.y + 19.0, Icon::ChevronLeft, 16.0, t.text);
    p.text(back.x + 40.0, Painter::baseline(back.y + 19.0, 15.0), "Voltar", 15.0, t.text, Weight::Regular);
    hits.push((back, Hit::SettingsBack));

    let items = [
        (Section::Appearance, Icon::Swatch, "Aparência"),
        (Section::Layout, Icon::Panel, "Layout"),
        (Section::About, Icon::Info, "Sobre o Ferro"),
    ];
    let mut y = area.y + 82.0;
    for (sec, ic, label) in items {
        let r = rect(side_x, y, side_w, 44.0);
        let selected = sec == section;
        if selected {
            p.rrect(r, 12.0, t.toolbar);
            p.stroke(r, 12.0, 1.0, t.border);
        } else if hover == Some(Hit::Section(sec)) {
            p.rrect(r, 12.0, t.field_hover);
        }
        let color = if selected { t.text } else { t.muted };
        p.icon(r.x + 24.0, r.y + 22.0, ic, 20.0, color);
        let weight = if selected { Weight::Semibold } else { Weight::Regular };
        p.text(r.x + 46.0, Painter::baseline(r.y + 22.0, 15.0), label, 15.0, color, weight);
        hits.push((r, Hit::Section(sec)));
        y += 50.0;
    }

    let panel = rect(side_x + side_w + 14.0, area.y + 10.0, area.x + area.w - (side_x + side_w + 14.0) - 10.0, area.h + 20.0);
    p.rrect(panel, 18.0, t.toolbar);
    p.stroke(panel, 18.0, 1.0, t.border);
    let x = panel.x + 44.0;
    let inner_w = panel.w - 88.0;
    let mut by = panel.y + 46.0;

    let page = match section {
        Section::Appearance => "Aparência",
        Section::Layout => "Layout",
        Section::About => "Sobre o Ferro",
    };
    let mut bx = x;
    for (i, part) in ["Início", "Configurações", page].iter().enumerate() {
        let last = i == 2;
        p.text(bx, Painter::baseline(by, 15.0), part, 15.0, if last { t.text } else { t.muted }, Weight::Regular);
        bx += p.measure(part, 15.0, Weight::Regular) + 18.0;
        if !last {
            p.icon(bx - 4.0, by, Icon::ChevronRight, 14.0, t.muted);
            bx += 18.0;
        }
    }
    by += 78.0;
    p.text(x, by, page, 38.0, t.text, Weight::Semibold);

    match section {
        Section::Appearance => {
            by += 64.0;
            p.text(x, by, "Tema", 26.0, t.text, Weight::Semibold);
            by += 30.0;
            p.text(x, by, "Cores do navegador, das abas e da busca.", 15.0, t.muted, Weight::Regular);
            let (cw, ch, gap) = (184.0, 116.0, 20.0);
            let per_row = (((inner_w + gap) / (cw + gap)).floor() as usize).max(1);
            let top = by + 28.0;
            for (i, th) in THEMES.iter().enumerate() {
                let card = rect(
                    x + (i % per_row) as f32 * (cw + gap),
                    top + (i / per_row) as f32 * (ch + 50.0),
                    cw,
                    ch,
                );
                theme_preview(p, card, th);
                if i == current {
                    ring(p, card, t.accent, 2.0);
                } else if hover == Some(Hit::Theme(i)) {
                    ring(p, card, t.muted, 1.0);
                }
                p.text(card.x, card.y + ch + 26.0, th.name, 15.0, t.text, Weight::Regular);
                hits.push((rect(card.x, card.y, cw, ch + 34.0), Hit::Theme(i)));
            }
        }
        Section::Layout => {
            by += 64.0;
            p.text(x, by, "Onde ficam as abas", 26.0, t.text, Weight::Semibold);
            by += 30.0;
            p.text(x, by, "Mude quando quiser; as abas abertas continuam onde estão.", 15.0, t.muted, Weight::Regular);
            let (cw, ch, gap) = (250.0, 156.0, 24.0);
            let per_row = (((inner_w + gap) / (cw + gap)).floor() as usize).max(1);
            let top = by + 28.0;
            let modes = [
                (LayoutMode::Sidebar, "Lateral", "Trilho de ícones e lista de abas"),
                (LayoutMode::Capsules, "Cápsulas", "Abas em pílulas no topo"),
            ];
            for (i, (mode, name, desc)) in modes.into_iter().enumerate() {
                let card = rect(
                    x + (i % per_row) as f32 * (cw + gap),
                    top + (i / per_row) as f32 * (ch + 70.0),
                    cw,
                    ch,
                );
                layout_preview(p, card, t, mode);
                if mode == layout {
                    ring(p, card, t.accent, 2.0);
                } else if hover == Some(Hit::Layout(mode)) {
                    ring(p, card, t.muted, 1.0);
                }
                p.text(card.x, card.y + ch + 26.0, name, 15.0, t.text, Weight::Semibold);
                p.text(card.x, card.y + ch + 48.0, desc, 13.0, t.muted, Weight::Regular);
                hits.push((rect(card.x, card.y, cw, ch + 56.0), Hit::Layout(mode)));
            }
        }
        Section::About => {
            by += 52.0;
            let lines: [(&str, bool); 10] = [
                ("Versão 0.1 (marco M1)", true),
                ("Navegador com motor próprio, escrito do zero e 100% em Rust.", false),
                ("", false),
                ("•  HTML, DOM, CSS, cascata, layout, SVG e pintura: código próprio", false),
                ("•  Uma thread de renderização por aba", false),
                ("•  Rede e TLS 100% Rust (rustls + RustCrypto)", false),
                ("•  Imagens PNG, JPEG, GIF, WebP e SVG", false),
                ("•  Ícones: Mage Icons (Apache 2.0)", false),
                ("", false),
                ("Próximo marco: flexbox, grid, floats e tabelas (M2).", false),
            ];
            for (line, strong) in lines {
                let w = if strong { Weight::Semibold } else { Weight::Regular };
                p.text(x, by, line, 15.0, if strong { t.text } else { t.muted }, w);
                by += 28.0;
            }
        }
    }
}

