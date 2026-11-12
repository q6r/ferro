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

