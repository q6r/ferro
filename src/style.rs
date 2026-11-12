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

/// Propriedades customizadas (`--x`) visíveis num elemento: as declaradas
/// nele e, por herança, as dos ancestrais. Encadeadas, para não copiar o
/// mapa inteiro a cada elemento que declara uma.
#[derive(Debug, Default)]
pub struct CustomVars {
    own: ferro_js::fxhash::FxHashMap<Rc<str>, Rc<str>>,
    parent: Option<Rc<CustomVars>>,
    depth: u32,
}

impl CustomVars {
    /// Novo elo com as declarações do elemento sobre as herdadas.
    fn child(parent: &Rc<CustomVars>, own: ferro_js::fxhash::FxHashMap<Rc<str>, Rc<str>>) -> Rc<CustomVars> {
        if parent.own.is_empty() && parent.parent.is_none() {
            return Rc::new(CustomVars { own, parent: None, depth: 0 });
        }
        if parent.depth >= 12 {
            // corrente comprida: achata (buscas continuam rápidas)
            let mut all = parent.flatten();
            all.extend(own);
            return Rc::new(CustomVars { own: all, parent: None, depth: 0 });
        }
        Rc::new(CustomVars { own, parent: Some(parent.clone()), depth: parent.depth + 1 })
    }

    pub fn get(&self, name: &str) -> Option<&Rc<str>> {
        let mut c = self;
        loop {
            if let Some(v) = c.own.get(name) {
                return Some(v);
            }
            c = c.parent.as_deref()?;
        }
    }

    /// Todas as visíveis (as do elemento vencem as herdadas).
    pub fn flatten(&self) -> ferro_js::fxhash::FxHashMap<Rc<str>, Rc<str>> {
        let mut out = match &self.parent {
            Some(p) => p.flatten(),
            None => Default::default(),
        };
        for (k, v) in &self.own {
            out.insert(k.clone(), v.clone());
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.own.is_empty() && self.parent.as_ref().is_none_or(|p| p.is_empty())
    }
}

/// Lados na ordem do CSS: top, right, bottom, left.
pub const TOP: usize = 0;
pub const RIGHT: usize = 1;
pub const BOTTOM: usize = 2;
pub const LEFT: usize = 3;

impl ComputedStyle {
    pub fn initial() -> Self {
        ComputedStyle {
            color: Color::BLACK,
            font_size: 16.0,
            font_weight: 400,
            italic: false,
            font_family: FontFamily::Serif,
            line_height: LineHeight::Normal,
            text_align: TextAlign::Left,
            white_space: WhiteSpace::Normal,
            text_transform: TextTransform::None,
            list_style: ListStyle::Disc,
            cursor: Cursor::Auto,
            pointer_events: true,
            visible: true,
            custom: Rc::new(CustomVars::default()),
            display: Display::Inline,
            background: Color::TRANSPARENT,
            margin: [Len::Px(0.0); 4],
            padding: [Len::Px(0.0); 4],
            border_width: [3.0; 4],
            border_style: [false; 4],
            border_color: [None; 4],
            width: Len::Auto,
            height: Len::Auto,
            min_height: Len::Auto,
            max_width: Len::Auto,
            max_height: Len::Auto,
            aspect_ratio: None,
            line_clamp: None,
            translate: None,
            transform: None,
            transform_origin: (Len::Percent(50.0), Len::Percent(50.0)),
            box_shadow: None,
            text_shadow: None,
            content_hidden: false,
            border_box: false,
            underline: false,
            line_through: false,
            position: Position::Static,
            left: Len::Auto,
            top: Len::Auto,
            clipped: false,
            overflow_clip: false,
            scroll_x: false,
            scroll_y: false,
            right: Len::Auto,
            bottom: Len::Auto,
            z_index: None,
            opacity: 1.0,
            min_width: Len::Auto,
            flex_direction: FlexDirection::Row,
            flex_wrap: false,
            justify_content: Justify::Start,
            align_items: Align::Stretch,
            align_self: None,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: FlexBasis::Auto,
            order: 0,
            row_gap: Len::Px(0.0),
            column_gap: Len::Px(0.0),
            float: Float::None,
            clear: Clear::None,
            vertical_align: VAlign::Baseline,
            border_spacing: 0.0,
            border_collapse: false,
            grid_columns: GridTemplate::default(),
            grid_rows: GridTemplate::default(),
            grid_areas: None,
            grid_area: None,
            grid_col: (GridLine::Auto, GridLine::Auto),
            grid_row: (GridLine::Auto, GridLine::Auto),
            border_radius: Len::Px(0.0),
            bg_image: None,
            bg_size: BgSize::Auto,
            bg_pos: (Len::Percent(0.0), Len::Percent(0.0)),
            bg_repeat: (true, true),
            content: None,
            font_names: Rc::new(Vec::new()),
            mask_image: None,
            mask_size: BgSize::Auto,
            mask_pos: (Len::Percent(0.0), Len::Percent(0.0)),
            mask_repeat: (true, true),
        }
    }

    /// Estilo novo que herda as propriedades herdáveis de `parent`.
    pub fn inherit_from(parent: &ComputedStyle) -> Self {
        ComputedStyle {
            color: parent.color,
            font_size: parent.font_size,
            font_weight: parent.font_weight,
            italic: parent.italic,
            font_family: parent.font_family,
            line_height: parent.line_height,
            text_align: parent.text_align,
            white_space: parent.white_space,
            text_transform: parent.text_transform,
            list_style: parent.list_style,
            cursor: parent.cursor,
            pointer_events: parent.pointer_events,
            visible: parent.visible,
            custom: parent.custom.clone(),
            border_spacing: parent.border_spacing,
            border_collapse: parent.border_collapse,
            font_names: parent.font_names.clone(),
            text_shadow: parent.text_shadow,
            ..ComputedStyle::initial()
        }
    }

    /// Largura da borda efetiva (zero se o estilo for none/hidden).
    pub fn border(&self, side: usize) -> f32 {
        if self.border_style[side] { self.border_width[side] } else { 0.0 }
    }

    pub fn border_color(&self, side: usize) -> Color {
        self.border_color[side].unwrap_or(self.color)
    }

    pub fn line_height_px(&self, normal: f32) -> f32 {
        match self.line_height {
            LineHeight::Normal => normal,
            LineHeight::Number(n) => n * self.font_size,
            LineHeight::Px(v) => v,
        }
    }

    /// Fora do fluxo normal (position absolute ou fixed).
    pub fn is_out_of_flow(&self) -> bool {
        matches!(self.position, Position::Absolute | Position::Fixed)
    }

    pub fn is_positioned(&self) -> bool {
        self.position != Position::Static
    }

    /// Elementos que o CSS esconde da tela mas deixa para leitores de tela
    /// (padrão `.sr-only`/`.visually-hidden`). Sem `position` real ainda,
    /// é melhor não desenhá-los.
    pub fn visually_hidden(&self) -> bool {
        if !matches!(self.position, Position::Absolute | Position::Fixed) {
            return false;
        }
        let tiny = |l: Len| matches!(l, Len::Px(v) if v <= 1.0);
        let offscreen = |l: Len| matches!(l, Len::Px(v) if v <= -500.0);
        self.clipped || tiny(self.width) || tiny(self.height) || offscreen(self.left) || offscreen(self.top)
    }
}

// --------------------------------------------------------------------- cores

