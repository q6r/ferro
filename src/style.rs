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

const NAMED_COLORS: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4), ("azure", 0xf0ffff), ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4), ("black", 0x000000), ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e), ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c), ("cyan", 0x00ffff),
    ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b), ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc), ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3), ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969), ("dimgrey", 0x696969), ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700), ("goldenrod", 0xdaa520), ("gray", 0x808080),
    ("green", 0x008000), ("greenyellow", 0xadff2f), ("grey", 0x808080),
    ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa), ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6), ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a), ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899), ("lightslategrey", 0x778899), ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa), ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371), ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc), ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5), ("navajowhite", 0xffdead), ("navy", 0x000080),
    ("oldlace", 0xfdf5e6), ("olive", 0x808000), ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093), ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f), ("pink", 0xffc0cb), ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
    ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513), ("salmon", 0xfa8072), ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee), ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd),
    ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f), ("steelblue", 0x4682b4), ("tan", 0xd2b48c),
    ("teal", 0x008080), ("thistle", 0xd8bfd8), ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3),
    ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
    // cores de sistema mais comuns
    ("canvas", 0xffffff), ("canvastext", 0x000000), ("linktext", 0x0000ee),
    ("buttonface", 0xefefef), ("buttontext", 0x000000), ("graytext", 0x808080),
];

pub fn parse_color(v: &str, current: Color) -> Option<Color> {
    let v = v.trim().to_ascii_lowercase();
    if v == "transparent" {
        return Some(Color::TRANSPARENT);
    }
    if v == "currentcolor" {
        return Some(current);
    }
    if let Some(hex) = v.strip_prefix('#') {
        let d: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|x| x as u8))
            .collect::<Option<Vec<u8>>>()?;
        return match d.len() {
            3 | 4 => Some(Color {
                r: d[0] * 17,
                g: d[1] * 17,
                b: d[2] * 17,
                a: if d.len() == 4 { d[3] * 17 } else { 255 },
            }),
            6 | 8 => Some(Color {
                r: d[0] * 16 + d[1],
                g: d[2] * 16 + d[3],
                b: d[4] * 16 + d[5],
                a: if d.len() == 8 { d[6] * 16 + d[7] } else { 255 },
            }),
            _ => None,
        };
    }
    // light-dark(clara, escura): o motor usa o esquema claro
    if let Some(inner) = v.strip_prefix("light-dark(").and_then(|s| s.strip_suffix(')')) {
        return parse_color(css::split_top_level(inner, ',').first()?, current);
    }
    if let Some(open) = v.find('(') {
        let func = &v[..open];
        let inner = v[open + 1..].trim_end_matches(')');
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let alpha = match parts.get(3) {
            Some(a) => (number_or_percent(a, 1.0)? * 255.0).round().clamp(0.0, 255.0) as u8,
            None => 255,
        };
        return match func {
            "rgb" | "rgba" => {
                let ch = |s: &str| number_or_percent(s, 255.0).map(|x| x.round().clamp(0.0, 255.0) as u8);
                Some(Color {
                    r: ch(parts[0])?,
                    g: ch(parts[1])?,
                    b: ch(parts[2])?,
                    a: alpha,
                })
            }
            "hsl" | "hsla" => {
                let h: f32 = parts[0].trim_end_matches("deg").parse().ok()?;
                let s = number_or_percent(parts[1], 1.0)?.clamp(0.0, 1.0);
                let l = number_or_percent(parts[2], 1.0)?.clamp(0.0, 1.0);
                let (r, g, b) = hsl_to_rgb(h, s, l);
                Some(Color { r, g, b, a: alpha })
            }
            _ => None,
        };
    }
    NAMED_COLORS
        .iter()
        .find(|(n, _)| *n == v)
        .map(|(_, c)| Color::rgb((c >> 16) as u8, (c >> 8) as u8, *c as u8))
}

/// "50%" → 0.5 * escala; "0.5" → 0.5 (escala 1) ou o próprio número.
fn number_or_percent(s: &str, scale: f32) -> Option<f32> {
    if let Some(p) = s.strip_suffix('%') {
        Some(p.parse::<f32>().ok()? / 100.0 * scale)
    } else {
        s.parse().ok()
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0) / 360.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0
        }
        if t > 1.0 {
            t -= 1.0
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

// ------------------------------------------------------------------ medidas

/// Contexto para resolver unidades relativas.
#[derive(Clone, Copy)]
struct Units {
    em: f32,
    rem: f32,
    vw: f32,
    vh: f32,
}

fn parse_len(v: &str, u: &Units) -> Option<Len> {
    let v = v.trim().to_ascii_lowercase();
    if v == "auto" {
        return Some(Len::Auto);
    }
    if let Some(inner) = v.strip_prefix("calc(").and_then(|s| s.strip_suffix(')')) {
        return parse_calc(inner, u);
    }
    let num_end = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e'))
        .unwrap_or(v.len());
    // "e" pode ser início de "em": recua se o que vem depois não for número
    let (mut num, mut unit) = v.split_at(num_end);
    if num.ends_with('e') {
        num = &v[..num_end - 1];
        unit = &v[num_end - 1..];
    }
    let n: f32 = num.parse().ok()?;
    let px = match unit {
        "px" => n,
        "" if n == 0.0 => 0.0,
        "%" => return Some(Len::Percent(n)),
        "em" => n * u.em,
        "rem" => n * u.rem,
        "ex" | "ch" => n * u.em * 0.5,
        "pt" => n * 4.0 / 3.0,
        "pc" => n * 16.0,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "q" => n * 96.0 / 101.6,
        "vw" => n * u.vw / 100.0,
        "vh" => n * u.vh / 100.0,
        "vmin" => n * u.vw.min(u.vh) / 100.0,
        "vmax" => n * u.vw.max(u.vh) / 100.0,
        _ => return None,
    };
    Some(Len::Px(px))
}

/// calc() simples: soma/subtração de termos em px/em/rem/vw (sem %).
fn parse_calc(inner: &str, u: &Units) -> Option<Len> {
    let mut total = 0.0;
    let mut sign = 1.0;
    for tok in inner.split_whitespace() {
        match tok {
            "+" => sign = 1.0,
            "-" => sign = -1.0,
            _ => match parse_len(tok, u)? {
                Len::Px(v) => total += sign * v,
                _ => return None,
            },
        }
    }
    Some(Len::Px(total))
}

/// 1 a 4 valores → [top, right, bottom, left].
fn expand_sides<T: Copy>(vals: &[T]) -> Option<[T; 4]> {
    match vals.len() {
        1 => Some([vals[0]; 4]),
        2 => Some([vals[0], vals[1], vals[0], vals[1]]),
        3 => Some([vals[0], vals[1], vals[2], vals[1]]),
        4 => Some([vals[0], vals[1], vals[2], vals[3]]),
        _ => None,
    }
}

fn split_values(v: &str) -> Vec<String> {
    let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
    css::split_top_level(&v, ' ')
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn border_width_value(v: &str, u: &Units) -> Option<f32> {
    match v {
        "thin" => Some(1.0),
        "medium" => Some(3.0),
        "thick" => Some(5.0),
        _ => match parse_len(v, u)? {
            Len::Px(p) => Some(p.max(0.0)),
            _ => None,
        },
    }
}

fn border_style_value(v: &str) -> Option<bool> {
    match v {
        "none" | "hidden" => Some(false),
        "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => Some(true),
        _ => None,
    }
}

// ------------------------------------------------------------------ cascata

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    UserAgent,
    Author,
}

type FxMap<K, V> = ferro_js::fxhash::FxHashMap<K, V>;

/// Bits do filtro de ancestrais (filtro de Bloom com 1024 posições).
const BLOOM_WORDS: usize = 16;
type Bloom = [u64; BLOOM_WORDS];

/// Hash de uma característica de seletor: tag, id ou classe.
fn feature_hash(kind: u8, s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5 ^ kind as u32;
    for b in s.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h
}

fn bloom_set(f: &mut Bloom, h: u32) {
    let bit = (h & 1023) as usize;
    f[bit / 64] |= 1 << (bit % 64);
}

fn bloom_has(f: &Bloom, h: u32) -> bool {
    let bit = (h & 1023) as usize;
    f[bit / 64] & (1 << (bit % 64)) != 0
}

/// Uma regra no índice: onde está (folha, regra, seletor) e quanto pesa.
#[derive(Clone)]
struct Entry {
    sheet: u32,
    rule: u32,
    sel: u32,
    origin: Origin,
    spec: Specificity,
    order: usize,
    /// Tags/ids/classes que algum ancestral do elemento precisa ter.
    ancestors: Box<[u32]>,
}

/// Índice das regras pela parte mais à direita do seletor, para não testar
/// todas as regras em todos os elementos.
#[derive(Default)]
struct Buckets {
    by_id: FxMap<Box<str>, Vec<u32>>,
    by_class: FxMap<Box<str>, Vec<u32>>,
    by_tag: FxMap<Box<str>, Vec<u32>>,
    by_attr: FxMap<Box<str>, Vec<u32>>,
    universal: Vec<u32>,
    len: usize,
}

impl Buckets {
    fn insert(&mut self, last: &css::Compound, entry: u32) {
        self.len += 1;
        if let Some(id) = &last.id {
            self.by_id.entry(id.as_str().into()).or_default().push(entry);
        } else if let Some(class) = last.classes.first() {
            self.by_class.entry(class.as_str().into()).or_default().push(entry);
        } else if let Some(tag) = &last.tag {
            self.by_tag.entry(tag.as_str().into()).or_default().push(entry);
        } else if let Some(a) = last.attrs.first() {
            self.by_attr.entry(a.name.to_ascii_lowercase().into()).or_default().push(entry);
        } else if last.pseudos.iter().any(|p| matches!(p, css::Pseudo::Root)) {
            self.by_tag.entry("html".into()).or_default().push(entry);
        } else {
            self.universal.push(entry);
        }
    }

    /// Regras que podem casar com o elemento (cada uma uma vez só).
    fn candidates(&self, el: &crate::dom::ElementData, out: &mut Vec<u32>) {
        out.clear();
        if let Some(id) = el.id()
            && let Some(v) = self.by_id.get(id)
        {
            out.extend(v);
        }
        // cada regra está num balde só; classe repetida ("a a") não repete regra
        for (i, class) in el.classes().enumerate() {
            if let Some(v) = self.by_class.get(class)
                && !el.classes().take(i).any(|c| c == class)
            {
                out.extend(v);
            }
        }
        if let Some(v) = self.by_tag.get(el.tag.as_str()) {
            out.extend(v);
        }
        if !self.by_attr.is_empty() {
            for (name, _) in &el.attrs {
                if let Some(v) = self.by_attr.get(name.as_str()) {
                    out.extend(v);
                }
            }
        }
        out.extend(&self.universal);
    }
}

/// Todas as regras das folhas, indexadas. Fica guardado entre um cálculo e
/// outro enquanto as folhas forem as mesmas.
struct RuleIndex {
    sheets: Vec<(Rc<Stylesheet>, Origin)>,
    entries: Vec<Entry>,
    normal: Buckets,
    pseudo: Buckets,
}

thread_local! {
    static INDEX_CACHE: std::cell::RefCell<Option<Rc<RuleIndex>>> = const { std::cell::RefCell::new(None) };
}

impl RuleIndex {
    /// O índice dessas folhas (reaproveita o último se forem as mesmas).
    fn for_sheets(sheets: &[(Rc<Stylesheet>, Origin)]) -> Rc<RuleIndex> {
        INDEX_CACHE.with(|c| {
            if let Some(ix) = c.borrow().as_ref()
                && ix.sheets.len() == sheets.len()
                && ix.sheets.iter().zip(sheets).all(|(a, b)| Rc::ptr_eq(&a.0, &b.0) && a.1 == b.1)
            {
                return ix.clone();
            }
            let ix = Rc::new(RuleIndex::build(sheets));
            if std::env::var_os("FERRO_TEMPOS").is_some() {
                eprintln!(
                    "[ferro] índice de regras: {} seletores ({} testados em todo elemento)",
                    ix.entries.len(),
                    ix.normal.universal.len()
                );
            }
            *c.borrow_mut() = Some(ix.clone());
            ix
        })
    }

    fn build(sheets: &[(Rc<Stylesheet>, Origin)]) -> Self {
        let mut idx = RuleIndex {
            sheets: sheets.to_vec(),
            entries: Vec::new(),
            normal: Buckets::default(),
            pseudo: Buckets::default(),
        };
        let mut order = 0;
        for (si, (sheet, origin)) in sheets.iter().enumerate() {
            for (ri, rule) in sheet.rules.iter().enumerate() {
                for (xi, sel) in rule.selectors.iter().enumerate() {
                    // pseudo-classe que nunca casa (:visited, ::first-line...): fora
                    if sel.compounds.iter().any(|c| c.pseudos.iter().any(|p| matches!(p, css::Pseudo::Never))) {
                        continue;
                    }
                    // compostos ligados por descendente/filho são ancestrais do elemento
                    let mut ancestors = Vec::new();
                    for (c, comb) in sel.compounds.iter().zip(&sel.combinators) {
                        if matches!(comb, css::Combinator::Descendant | css::Combinator::Child) {
                            if let Some(id) = &c.id {
                                ancestors.push(feature_hash(b'#', id));
                            }
                            for cl in &c.classes {
                                ancestors.push(feature_hash(b'.', cl));
                            }
                            if let Some(t) = &c.tag {
                                ancestors.push(feature_hash(b't', t));
                            }
                        }
                    }
                    let id = idx.entries.len() as u32;
                    idx.entries.push(Entry {
                        sheet: si as u32,
                        rule: ri as u32,
                        sel: xi as u32,
                        origin: *origin,
                        spec: sel.specificity(),
                        order,
                        ancestors: ancestors.into_boxed_slice(),
                    });
                    order += 1;
                    let last = sel.compounds.last().unwrap();
                    if sel.pseudo_element.is_some() {
                        idx.pseudo.insert(last, id);
                    } else {
                        idx.normal.insert(last, id);
                    }
                }
            }
        }
        idx
    }

    fn rule(&self, e: &Entry) -> &css::Rule {
        &self.sheets[e.sheet as usize].0.rules[e.rule as usize]
    }
}

/// Estilos de todos os nós (índice = NodeId) e dos pseudo-elementos.
pub struct Styles {
    pub nodes: Vec<Option<Rc<ComputedStyle>>>,
    pub before: HashMap<NodeId, Rc<ComputedStyle>>,
    pub after: HashMap<NodeId, Rc<ComputedStyle>>,
}

impl std::ops::Index<NodeId> for Styles {
    type Output = Option<Rc<ComputedStyle>>;

    fn index(&self, i: NodeId) -> &Self::Output {
        &self.nodes[i]
    }
}

/// Calcula o estilo de todos os elementos. Índice = NodeId; nós de texto
/// recebem o estilo do pai.
pub fn compute(doc: &Document, sheets: &[(Rc<Stylesheet>, Origin)], media: &MediaContext) -> Styles {
    let index = RuleIndex::for_sheets(sheets);
    let mut styles = Styles {
        nodes: vec![None; doc.nodes.len()],
        before: HashMap::new(),
        after: HashMap::new(),
    };
    let root_style = Rc::new(ComputedStyle::initial());
    let mut cx = Cascade {
        doc,
        index: &index,
        media: *media,
        root_font: 16.0,
        own_bits: vec![None; doc.nodes.len()],
        ancestor_bits: vec![None; doc.nodes.len()],
        scratch: std::cell::RefCell::new(Vec::new()),
    };
    let mut stack: Vec<(NodeId, Rc<ComputedStyle>)> = doc
        .node(Document::ROOT)
        .children
        .iter()
        .rev()
        .map(|&c| (c, root_style.clone()))
        .collect();
    while let Some((node, parent)) = stack.pop() {
        let style = match &doc.node(node).data {
            NodeData::Element(_) => {
                cx.prepare_ancestors(node);
                let s = Rc::new(cx.style_for(node, &parent, None).0);
                if doc.node(node).parent == Some(Document::ROOT) {
                    cx.root_font = s.font_size;
                }
                if index.pseudo.len > 0 && s.display != Display::None {
                    for (pe, map) in [
                        (css::PseudoElement::Before, &mut styles.before),
                        (css::PseudoElement::After, &mut styles.after),
                    ] {
                        let (ps, matched) = cx.style_for(node, &s, Some(pe));
                        if matched && ps.content.is_some() && ps.display != Display::None {
                            map.insert(node, Rc::new(ps));
                        }
                    }
                }
                s
            }
            NodeData::Text(_) => parent.clone(),
            _ => continue,
        };
        for &c in doc.flat_children(node).iter().rev() {
            stack.push((c, style.clone()));
        }
        styles.nodes[node] = Some(style);
    }
    styles
}

