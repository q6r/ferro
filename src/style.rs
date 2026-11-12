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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlexDirection {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl FlexDirection {
    pub fn is_column(self) -> bool {
        matches!(self, FlexDirection::Column | FlexDirection::ColumnReverse)
    }
}

/// justify-content
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Justify {
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// align-items / align-self
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Align {
    Stretch,
    Start,
    End,
    Center,
    Baseline,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlexBasis {
    Auto,
    Content,
    Len(Len),
}

#[derive(Debug, Clone)]
pub struct ComputedStyle {
    // herdadas
    pub color: Color,
    pub font_size: f32,
    pub font_weight: u16,
    pub italic: bool,
    pub font_family: FontFamily,
    pub line_height: LineHeight,
    pub text_align: TextAlign,
    pub white_space: WhiteSpace,
    pub text_transform: TextTransform,
    pub list_style: ListStyle,
    pub visible: bool,
    pub custom: Rc<CustomVars>,
    pub cursor: Cursor,
    /// pointer-events (false = none: o mouse atravessa o elemento).
    pub pointer_events: bool,
    // não herdadas
    pub display: Display,
    pub background: Color,
    pub margin: [Len; 4],
    pub padding: [Len; 4],
    pub border_width: [f32; 4],
    pub border_style: [bool; 4],
    pub border_color: [Option<Color>; 4],
    pub width: Len,
    pub height: Len,
    pub min_height: Len,
    pub max_width: Len,
    pub max_height: Len,
    /// aspect-ratio (largura / altura).
    pub aspect_ratio: Option<f32>,
    /// -webkit-line-clamp: no máximo N linhas (o resto é cortado).
    pub line_clamp: Option<u32>,
    /// transform/translate: deslocamento [px x, % x, px y, % y].
    pub translate: Option<[f32; 4]>,
    /// transform com rotação/escala/inclinação/matriz (aplicado na pintura).
    pub transform: Option<Rc<Vec<TransformFn>>>,
    /// transform-origin (padrão: 50% 50%).
    pub transform_origin: (Len, Len),
    /// box-shadow (as de fora; `inset` é ignorada).
    pub box_shadow: Option<Rc<Vec<Shadow>>>,
    /// text-shadow (herdada; só a primeira sombra).
    pub text_shadow: Option<Shadow>,
    /// content-visibility: hidden (a caixa fica, o conteúdo não é desenhado).
    pub content_hidden: bool,
    pub border_box: bool,
    pub underline: bool,
    pub line_through: bool,
    pub position: Position,
    pub left: Len,
    pub top: Len,
    pub clipped: bool,
    pub overflow_clip: bool,
    /// overflow-x / overflow-y: auto ou scroll (o usuário pode rolar).
    pub scroll_x: bool,
    pub scroll_y: bool,
    pub right: Len,
    pub bottom: Len,
    pub z_index: Option<i32>,
    pub opacity: f32,
    pub min_width: Len,
    // flexbox
    pub flex_direction: FlexDirection,
    pub flex_wrap: bool,
    pub justify_content: Justify,
    pub align_items: Align,
    pub align_self: Option<Align>,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: FlexBasis,
    pub order: i32,
    pub row_gap: Len,
    pub column_gap: Len,
    pub float: Float,
    pub clear: Clear,
    pub vertical_align: VAlign,
    // tabelas (herdadas)
    pub border_spacing: f32,
    pub border_collapse: bool,
    // grid
    pub grid_columns: GridTemplate,
    pub grid_rows: GridTemplate,
    pub grid_areas: Option<Rc<Vec<Vec<String>>>>,
    pub grid_area: Option<String>,
    pub grid_col: (GridLine, GridLine),
    pub grid_row: (GridLine, GridLine),
    pub border_radius: Len,
    pub bg_image: Option<BgImage>,
    pub bg_size: BgSize,
    pub bg_pos: (Len, Len),
    pub bg_repeat: (bool, bool),
    /// `content` de ::before/::after (None = none/normal).
    pub content: Option<Rc<Vec<ContentPart>>>,
    /// Nomes de `font-family`, em minúsculas (para as fontes web). Herdado.
    pub font_names: Rc<Vec<Rc<str>>>,
    /// mask-image: a imagem recorta o fundo (ícones pintados com a cor do fundo).
    pub mask_image: Option<Rc<str>>,
    pub mask_size: BgSize,
    pub mask_pos: (Len, Len),
    pub mask_repeat: (bool, bool),
}

