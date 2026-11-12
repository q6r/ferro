//! Temas da interface, montados a partir das paletas em `assets/pallets`
//! e do kit de configurações (tema claro).

use crate::style::Color;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    /// Faixa das abas (fundo da janela).
    pub frame: Color,
    /// Barra de ferramentas e aba ativa.
    pub toolbar: Color,
    pub tab_hover: Color,
    /// Fundo da omnibox e dos campos.
    pub field: Color,
    pub field_hover: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    /// Texto sobre a cor de destaque.
    pub on_accent: Color,
    /// Nova aba: fundo e pontos da grade.
    pub ntp_bg: Color,
    pub ntp_dot: Color,
    pub card: Color,
    pub card_hover: Color,
    /// Menus e a lista de sugestões.
    pub popup: Color,
    pub selection: Color,
}

const fn hex(v: u32) -> Color {
    Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const THEMES: [Theme; 5] = [
    // Paleta "Royal Charcoal": #1e2023 #47484c #4c4e51 #1e1f22
    Theme {
        id: "carvao",
        name: "Carvão",
        dark: true,
        frame: hex(0x1e1f22),
        toolbar: hex(0x2a2b2f),
        tab_hover: hex(0x323438),
        field: hex(0x1e2023),
        field_hover: hex(0x26282b),
        border: hex(0x47484c),
        text: hex(0xececee),
        muted: hex(0x9c9da2),
        accent: hex(0xd6d7db),
        on_accent: hex(0x1e1f22),
        ntp_bg: hex(0x1e1f22),
        ntp_dot: hex(0x2e3033),
        card: hex(0x26282b),
        card_hover: hex(0x323438),
        popup: hex(0x2a2b2f),
        selection: hex(0x4c4e51),
    },
    // Paleta moca: #87776C → #090909
    Theme {
        id: "moca",
        name: "Moca",
        dark: true,
        frame: hex(0x090909),
        toolbar: hex(0x1c1917),
        tab_hover: hex(0x26221f),
        field: hex(0x0e0d0c),
        field_hover: hex(0x181614),
        border: hex(0x3b3430),
        text: hex(0xf2ece7),
        muted: hex(0xa89a90),
        accent: hex(0x87776c),
        on_accent: hex(0xffffff),
        ntp_bg: hex(0x110f0e),
        ntp_dot: hex(0x231f1c),
        card: hex(0x1c1917),
        card_hover: hex(0x29241f),
        popup: hex(0x1c1917),
        selection: hex(0x4a403a),
    },
    // Paleta crepúsculo: #190019 #2B124C #522B5B #854F6C #DFB6B2 #FBE4D8
    Theme {
        id: "crepusculo",
        name: "Crepúsculo",
        dark: true,
        frame: hex(0x190019),
        toolbar: hex(0x2b124c),
        tab_hover: hex(0x3a1d58),
        field: hex(0x190019),
        field_hover: hex(0x220a2c),
        border: hex(0x522b5b),
        text: hex(0xfbe4d8),
        muted: hex(0xc9a3a6),
        accent: hex(0xdfb6b2),
        on_accent: hex(0x190019),
        ntp_bg: hex(0x190019),
        ntp_dot: hex(0x2b124c),
        card: hex(0x2b124c),
        card_hover: hex(0x3a1d58),
        popup: hex(0x2b124c),
        selection: hex(0x522b5b),
    },
    // Paleta névoa: #0B162A #2A2D40 #4A4959 #83828A #BCBCBC
    Theme {
        id: "nevoa",
        name: "Névoa",
        dark: true,
        frame: hex(0x0b162a),
        toolbar: hex(0x1a2135),
        tab_hover: hex(0x2a2d40),
        field: hex(0x0b162a),
        field_hover: hex(0x131d33),
        border: hex(0x4a4959),
        text: hex(0xe9e9ee),
        muted: hex(0xa9a9b1),
        accent: hex(0xbcbcbc),
        on_accent: hex(0x0b162a),
        ntp_bg: hex(0x0b162a),
        ntp_dot: hex(0x1a2438),
        card: hex(0x1a2135),
        card_hover: hex(0x2a2d40),
        popup: hex(0x1a2135),
        selection: hex(0x4a4959),
    },
    // Kit de configurações (claro)
    Theme {
        id: "claro",
        name: "Claro",
        dark: false,
        frame: hex(0xf3f3f3),
        toolbar: hex(0xffffff),
        tab_hover: hex(0xe8e8e8),
        field: hex(0xf1f1f1),
        field_hover: hex(0xe9e9e9),
        border: hex(0xe1e1e1),
        text: hex(0x111111),
        muted: hex(0x6e6e6e),
        accent: hex(0x1a73e8),
        on_accent: hex(0xffffff),
        ntp_bg: hex(0xfafafa),
        ntp_dot: hex(0xe4e4e4),
        card: hex(0xffffff),
        card_hover: hex(0xf2f2f2),
        popup: hex(0xffffff),
        selection: hex(0xdbe8fd),
    },
];

pub fn by_id(id: &str) -> usize {
    THEMES.iter().position(|t| t.id == id).unwrap_or(0)
}

/// Mistura `a` com `b` (t = 0 → a, t = 1 → b).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: 255,
    }
}

pub fn with_alpha(c: Color, a: u8) -> Color {
    Color { a, ..c }
}

impl Theme {
    /// Trilho de ícones do layout lateral (preto no tema claro, como no kit).
    pub fn rail(&self) -> Color {
        if self.dark { self.toolbar } else { hex(0x151516) }
    }

    pub fn rail_active(&self) -> Color {
        if self.dark { self.selection } else { hex(0x3b3b3e) }
    }

    pub fn rail_icon(&self) -> Color {
        if self.dark { self.muted } else { hex(0x9d9da2) }
    }

    pub fn rail_icon_active(&self) -> Color {
        if self.dark { self.text } else { Color::WHITE }
    }

    /// Fundo da aba selecionada na lista e das cápsulas ativas.
    pub fn row_selected(&self) -> Color {
        if self.dark { self.toolbar } else { hex(0xe8e9eb) }
    }

    /// Vidro da paleta de busca: (topo, base, borda).
    pub fn glass(&self) -> (Color, Color, Color) {
        if self.dark {
            (
                with_alpha(mix(self.popup, Color::WHITE, 0.10), 150),
                with_alpha(self.popup, 196),
                Color { r: 255, g: 255, b: 255, a: 42 },
            )
        } else {
            (
                Color { r: 255, g: 255, b: 255, a: 205 },
                Color { r: 244, g: 244, b: 246, a: 222 },
                Color { r: 0, g: 0, b: 0, a: 26 },
            )
        }
    }
}
