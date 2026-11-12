//! Cascata e estilo computado: para cada elemento, junta as declarações que
//! casam (folha do agente de usuário, folhas do autor, atributo style),
//! ordena por origem/importância/especificidade/ordem e calcula os valores.

use std::collections::HashMap;
use std::rc::Rc;

use crate::css::{self, Declaration, MediaContext, Specificity, Stylesheet};
use crate::dom::{Document, NodeData, NodeId};

// --------------------------------------------------------------------- tipos

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const TRANSPARENT: Color = Color { r: 0, g: 0, b: 0, a: 0 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    pub fn is_transparent(&self) -> bool {
        self.a == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Display {
    None,
    Block,
    Inline,
    InlineBlock,
    InlineFlex,
    InlineGrid,
    ListItem,
    Flex,
    Grid,
    Table,
    TableRowGroup,
    TableRow,
    TableCell,
    TableCaption,
    Contents,
}

impl Display {
    pub fn is_block_level(self) -> bool {
        !matches!(
            self,
            Display::Inline
                | Display::InlineBlock
                | Display::InlineFlex
                | Display::InlineGrid
                | Display::None
                | Display::Contents
        )
    }

    /// Caixa em linha que por dentro é um bloco (inline-block/flex/grid).
    pub fn is_inline_block(self) -> bool {
        matches!(self, Display::InlineBlock | Display::InlineFlex | Display::InlineGrid)
    }

    pub fn is_flex(self) -> bool {
        matches!(self, Display::Flex | Display::InlineFlex)
    }

    pub fn is_grid(self) -> bool {
        matches!(self, Display::Grid | Display::InlineGrid)
    }
}

/// Gradiente linear: ângulo em graus (0 = para cima, 180 = para baixo) e
/// paradas de cor com posição de 0 a 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub angle: f32,
    pub stops: Vec<(Color, f32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BgImage {
    Url(Rc<str>),
    Linear(Rc<Gradient>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BgSize {
    Auto,
    Cover,
    Contain,
    Size(Len, Len),
}

