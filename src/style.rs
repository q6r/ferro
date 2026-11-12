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

/// Partes da propriedade `content` (::before/::after).
#[derive(Debug, Clone, PartialEq)]
pub enum ContentPart {
    Text(String),
    Attr(String),
    OpenQuote,
    CloseQuote,
}

/// Medida ainda não resolvida contra o bloco contêiner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Len {
    Auto,
    Px(f32),
    Percent(f32),
}

impl Len {
    pub fn resolve(self, base: f32) -> Option<f32> {
        match self {
            Len::Auto => None,
            Len::Px(v) => Some(v),
            Len::Percent(p) => Some(p / 100.0 * base),
        }
    }

    pub fn resolve_or_zero(self, base: f32) -> f32 {
        self.resolve(base).unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontFamily {
    Serif,
    Sans,
    Mono,
    /// system-ui: a fonte da interface do sistema (Segoe UI no Windows).
    System,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    Normal,
    Number(f32),
    Px(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhiteSpace {
    Normal,
    Pre,
    NoWrap,
    PreWrap,
    PreLine,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextTransform {
    None,
    Upper,
    Lower,
    Capitalize,
}

/// Ponteiro do mouse pedido pelo CSS (`cursor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cursor {
    #[default]
    Auto,
    Default,
    Pointer,
    Text,
    Move,
    NotAllowed,
    Grab,
    Grabbing,
    Wait,
    Progress,
    Help,
    Crosshair,
    EwResize,
    NsResize,
    NeswResize,
    NwseResize,
    ZoomIn,
    ZoomOut,
    None,
}

impl Cursor {
    /// Lê o valor de `cursor` (`url(...) x y, pointer` fica com a palavra do fim).
    pub fn parse(v: &str) -> Option<Cursor> {
        let last = v.rsplit(',').next()?.trim().to_ascii_lowercase();
        Some(match last.as_str() {
            "auto" => Cursor::Auto,
            "default" | "context-menu" | "alias" | "copy" | "cell" => Cursor::Default,
            "pointer" | "hand" => Cursor::Pointer,
            "text" | "vertical-text" => Cursor::Text,
            "move" | "all-scroll" => Cursor::Move,
            "not-allowed" | "no-drop" => Cursor::NotAllowed,
            "grab" | "-webkit-grab" => Cursor::Grab,
            "grabbing" | "-webkit-grabbing" => Cursor::Grabbing,
            "wait" => Cursor::Wait,
            "progress" => Cursor::Progress,
            "help" => Cursor::Help,
            "crosshair" => Cursor::Crosshair,
            "ew-resize" | "col-resize" | "e-resize" | "w-resize" => Cursor::EwResize,
            "ns-resize" | "row-resize" | "n-resize" | "s-resize" => Cursor::NsResize,
            "nesw-resize" | "ne-resize" | "sw-resize" => Cursor::NeswResize,
            "nwse-resize" | "nw-resize" | "se-resize" => Cursor::NwseResize,
            "zoom-in" | "-webkit-zoom-in" => Cursor::ZoomIn,
            "zoom-out" | "-webkit-zoom-out" => Cursor::ZoomOut,
            "none" => Cursor::None,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListStyle {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Float {
    None,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Clear {
    None,
    Left,
    Right,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VAlign {
    Baseline,
    Top,
    Middle,
    Bottom,
}

/// Um limite de trilha do grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Track {
    Len(Len),
    Fr(f32),
    Auto,
    MinContent,
    MaxContent,
}

/// Trilha do grid: minmax(min, max). Uma trilha simples tem min = max
/// (e `1fr` vira minmax(auto, 1fr)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSize {
    pub min: Track,
    pub max: Track,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GridTemplate {
    pub tracks: Vec<TrackSize>,
    /// repeat(auto-fill | auto-fit, ...): repete quantas couberem.
    pub auto_fill: Option<Vec<TrackSize>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridLine {
    Auto,
    Line(i32),
    Span(u32),
}

