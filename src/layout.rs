//! Layout: transforma o DOM estilizado numa árvore de caixas e calcula a
//! posição de cada uma.
//!
//! Implementado: fluxo de bloco e de linha (CSS 2.1) com floats e clear,
//! flexbox (linha e coluna, quebra, grow/shrink/basis, gap, justify/align,
//! margens auto, order), grid (trilhas fixas, fr, auto, minmax, repeat,
//! auto-fill, áreas nomeadas), tabelas (largura automática de colunas,
//! colspan, alinhamento vertical), position relative/absolute/fixed com
//! camadas por z-index, e opacity.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::dom::{Document, NodeData, NodeId};
use crate::fonts::{FontId, Fonts};
use crate::paint::{DisplayItem, ImageData, Radii};
use crate::style::{
    Align, BOTTOM, BgImage, BgSize, Clear, Color, ComputedStyle, ContentPart, Display, FlexBasis, Float, GridLine,
    Justify, LEFT, Len, ListStyle, Position, RIGHT, SizeKw, Styles, TOP, TextAlign, TextTransform, Track, TrackSize, VAlign,
    WhiteSpace,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Interseção (vazia: largura ou altura zero).
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = (self.x + self.w).min(o.x + o.w);
        let b = (self.y + self.h).min(o.y + o.h);
        Rect { x, y, w: (r - x).max(0.0), h: (b - y).max(0.0) }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

// ------------------------------------------------------------- árvore de caixas

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoxKind {
    Block(NodeId),
    Anonymous,
    Inline(NodeId),
    Text(NodeId),
    /// Elemento substituído (img, input, svg...): uma caixa indivisível na linha.
    Atomic(NodeId),
    /// inline-block / inline-flex / inline-grid: bloco por dentro, peça única na linha.
    InlineBlock(NodeId),
}

pub struct LayoutBox {
    pub kind: BoxKind,
    pub style: Rc<ComputedStyle>,
    pub children: Vec<LayoutBox>,
    /// Texto gerado (conteúdo de ::before/::after).
    pub text: Option<Rc<str>>,
    /// Caixa de pseudo-elemento.
    pub generated: bool,
}

impl LayoutBox {
    pub fn new(kind: BoxKind, style: Rc<ComputedStyle>, children: Vec<LayoutBox>) -> LayoutBox {
        LayoutBox {
            kind,
            style,
            children,
            text: None,
            generated: false,
        }
    }

    pub fn is_block_level(&self) -> bool {
        matches!(self.kind, BoxKind::Block(_) | BoxKind::Anonymous)
    }

    /// Posicionado com absolute/fixed: não ocupa espaço no fluxo.
    pub fn is_out_of_flow(&self) -> bool {
        matches!(self.kind, BoxKind::Block(_)) && self.style.is_out_of_flow()
    }

    pub fn is_float(&self) -> bool {
        matches!(self.kind, BoxKind::Block(_)) && self.style.float != Float::None && !self.style.is_out_of_flow()
    }

    /// Bloco que ocupa o seu lugar no fluxo normal (nem float nem posicionado).
    pub fn is_inflow_block(&self) -> bool {
        self.is_block_level() && !self.is_out_of_flow() && !self.is_float()
    }

    pub fn node(&self) -> Option<NodeId> {
        match self.kind {
            BoxKind::Block(n)
            | BoxKind::Inline(n)
            | BoxKind::Text(n)
            | BoxKind::Atomic(n)
            | BoxKind::InlineBlock(n) => Some(n),
            BoxKind::Anonymous => None,
        }
    }

    pub fn dump(&self, doc: &Document) -> String {
        let mut out = String::new();
        self.dump_into(doc, 0, &mut out);
        out
    }

    fn dump_into(&self, doc: &Document, depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth);
        let mut label = match self.kind {
            BoxKind::Block(n) => format!("bloco <{}>", doc.tag(n).unwrap_or("?")),
            BoxKind::Anonymous => "bloco anônimo".to_string(),
            BoxKind::Inline(n) => format!("em linha <{}>", doc.tag(n).unwrap_or("?")),
            BoxKind::Atomic(n) => format!("atômica <{}>", doc.tag(n).unwrap_or("?")),
            BoxKind::InlineBlock(n) => format!("inline-block <{}>", doc.tag(n).unwrap_or("?")),
            BoxKind::Text(n) => {
                let full = match &self.text {
                    Some(t) => t.to_string(),
                    None => doc.text_content(n),
                };
                let t = full.split_whitespace().collect::<Vec<_>>().join(" ");
                format!("texto \"{}\"", t.chars().take(60).collect::<String>())
            }
        };
        if self.generated {
            label.push_str(" [gerado]");
        }
        if !matches!(self.kind, BoxKind::Anonymous | BoxKind::Text(_)) {
            match self.style.display {
                Display::Flex | Display::InlineFlex => label.push_str(" [flex]"),
                Display::Grid | Display::InlineGrid => label.push_str(" [grid]"),
                Display::Table => label.push_str(" [tabela]"),
                _ => {}
            }
        }
        if self.is_out_of_flow() {
            label.push_str(" [fora do fluxo]");
        }
        if self.is_float() {
            label.push_str(" [float]");
        }
        out.push_str(&format!("{pad}{label}\n"));
        for c in &self.children {
            c.dump_into(doc, depth + 1, out);
        }
    }
}

fn has_inflow_blocks(b: &LayoutBox) -> bool {
    b.children.iter().any(LayoutBox::is_inflow_block)
}

const REPLACED: &[&str] = &[
    "img", "input", "select", "textarea", "svg", "video", "iframe", "canvas", "object", "embed", "audio",
];

pub fn build_box_tree(doc: &Document, styles: &Styles) -> Option<LayoutBox> {
    let html = doc
        .node(Document::ROOT)
        .children
        .iter()
        .copied()
        .find(|&c| doc.element(c).is_some())?;
    let mut root = build(doc, styles, html).pop()?;
    if !root.is_block_level() {
        root.kind = BoxKind::Block(html);
        wrap_inline_children(doc, &mut root);
    }
    Some(root)
}

fn build(doc: &Document, styles: &Styles, node: NodeId) -> Vec<LayoutBox> {
    let Some(style) = styles[node].clone() else {
        return Vec::new();
    };
    match &doc.node(node).data {
        NodeData::Text(t) if !t.is_empty() => vec![LayoutBox::new(BoxKind::Text(node), style, Vec::new())],
        NodeData::Element(el) => {
            if style.display == Display::None || style.visually_hidden() {
                return Vec::new();
            }
            let boxed = style.is_out_of_flow() || style.float != Float::None;
            if REPLACED.contains(&el.tag.as_str()) {
                if boxed {
                    // caixa posicionada/flutuante em volta do elemento
                    // substituído. Margem, padding, borda, fundo, sombra,
                    // transformação e limites de tamanho ficam só na de fora;
                    // a de dentro ocupa a área de conteúdo dela
                    let mut inner = (*style).clone();
                    inner.margin = [Len::Px(0.0); 4];
                    let pad = |l: Len| if let Len::Px(v) = l { v } else { 0.0 };
                    let hx = pad(style.padding[LEFT]) + pad(style.padding[RIGHT]) + style.border(LEFT) + style.border(RIGHT);
                    let vx = pad(style.padding[TOP]) + pad(style.padding[BOTTOM]) + style.border(TOP) + style.border(BOTTOM);
                    inner.width = match style.width {
                        Len::Px(w) => Len::Px(if style.border_box { (w - hx).max(0.0) } else { w }),
                        Len::Auto => Len::Auto,
                        _ => Len::Percent(100.0),
                    };
                    if let Len::Px(h) = style.height {
                        inner.height = Len::Px(if style.border_box { (h - vx).max(0.0) } else { h });
                    }
                    inner.border_box = false;
                    inner.padding = [Len::Px(0.0); 4];
                    inner.border_width = [0.0; 4];
                    inner.border_style = [false; 4];
                    inner.background = Color::TRANSPARENT;
                    inner.bg_image = None;
                    inner.box_shadow = None;
                    inner.transform = None;
                    inner.opacity = 1.0;
                    inner.min_width = Len::Auto;
                    inner.max_width = Len::Auto;
                    inner.min_height = Len::Auto;
                    inner.max_height = Len::Auto;
                    let atomic = LayoutBox::new(BoxKind::Atomic(node), Rc::new(inner), Vec::new());
                    return vec![LayoutBox::new(BoxKind::Block(node), style, vec![atomic])];
                }
                let atomic = LayoutBox::new(BoxKind::Atomic(node), style.clone(), Vec::new());
                return vec![atomic];
            }
            let mut children: Vec<LayoutBox> = Vec::new();
            if let Some(ps) = styles.before.get(&node) {
                children.extend(pseudo_box(doc, el, node, ps.clone()));
            }
            if !style.content_hidden {
                children.extend(doc.flat_children(node).iter().flat_map(|&c| build(doc, styles, c)));
            }
            if let Some(ps) = styles.after.get(&node) {
                children.extend(pseudo_box(doc, el, node, ps.clone()));
            }
            if style.display == Display::Contents {
                return children;
            }
            let inline_block = !boxed && style.display.is_inline_block();
            // Um elemento em linha que contém blocos vira bloco (simplificação
            // do "block-in-inline" do CSS); posicionados e floats também.
            let block = !inline_block
                && (boxed || style.display.is_block_level() || children.iter().any(LayoutBox::is_inflow_block));
            let kind = if inline_block {
                BoxKind::InlineBlock(node)
            } else if block {
                BoxKind::Block(node)
            } else {
                BoxKind::Inline(node)
            };
            let mut b = LayoutBox::new(kind, style, children);
            if block || inline_block {
                if b.style.display.is_flex() || b.style.display.is_grid() {
                    make_flex_items(doc, &mut b);
                } else if b.style.display == Display::Table {
                    wrap_table_children(doc, &mut b);
                } else {
                    wrap_inline_children(doc, &mut b);
                }
            }
            vec![b]
        }
        _ => Vec::new(),
    }
}

/// Caixa de ::before / ::after com o texto de `content`.
fn pseudo_box(doc: &Document, el: &crate::dom::ElementData, node: NodeId, s: Rc<ComputedStyle>) -> Option<LayoutBox> {
    let _ = doc;
    let parts = s.content.clone()?;
    let text: String = parts
        .iter()
        .map(|p| match p {
            ContentPart::Text(t) => t.clone(),
            ContentPart::Attr(a) => el.attr(a).unwrap_or("").to_string(),
            ContentPart::OpenQuote => "“".to_string(),
            ContentPart::CloseQuote => "”".to_string(),
        })
        .collect();
    let mut children = Vec::new();
    if !text.is_empty() {
        let mut t = LayoutBox::new(BoxKind::Text(node), s.clone(), Vec::new());
        t.text = Some(Rc::from(text.as_str()));
        t.generated = true;
        children.push(t);
    }
    let kind = if s.is_out_of_flow() || s.float != Float::None || s.display.is_block_level() {
        BoxKind::Block(node)
    } else if s.display.is_inline_block() {
        BoxKind::InlineBlock(node)
    } else {
        BoxKind::Inline(node)
    };
    let mut b = LayoutBox::new(kind, s, children);
    b.generated = true;
    Some(b)
}

fn anonymous_style(parent: &ComputedStyle) -> Rc<ComputedStyle> {
    let mut s = ComputedStyle::inherit_from(parent);
    s.display = Display::Block;
    Rc::new(s)
}

fn only_whitespace(doc: &Document, run: &[LayoutBox]) -> bool {
    run.iter().all(|c| match c.kind {
        BoxKind::Text(n) => {
            matches!(c.style.white_space, WhiteSpace::Normal | WhiteSpace::NoWrap)
                && match &c.text {
                    Some(t) => t.chars().all(is_collapsible),
                    None => doc.text_content(n).chars().all(is_collapsible),
                }
        }
        _ => false,
    })
}

/// Se um bloco tem filhos de bloco e em linha misturados, agrupa os em linha
/// (e os floats e posicionados entre eles) em blocos anônimos (CSS 2.1 §9.2.1.1).
/// Filhos de uma tabela que não são partes de tabela (texto, elementos em
/// linha, blocos comuns) vão para uma célula anônima, como manda o CSS
/// (§17.2.1); sem isso a tabela os ignorava e ficava com largura 0.
fn wrap_table_children(doc: &Document, b: &mut LayoutBox) {
    let part = |c: &LayoutBox| {
        matches!(
            c.style.display,
            Display::TableCaption | Display::TableRowGroup | Display::TableRow | Display::TableCell
        ) || c.is_out_of_flow()
    };
    if b.children.iter().all(part) {
        return;
    }
    let mut cell = ComputedStyle::inherit_from(&b.style);
    cell.display = Display::TableCell;
    let cell = Rc::new(cell);
    let mut out = Vec::new();
    let mut run: Vec<LayoutBox> = Vec::new();
    let flush = |run: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>| {
        if run.is_empty() {
            return;
        }
        let children = std::mem::take(run);
        if !only_whitespace(doc, &children) {
            let mut c = LayoutBox::new(BoxKind::Anonymous, cell.clone(), children);
            wrap_inline_children(doc, &mut c);
            out.push(c);
        }
    };
    for child in std::mem::take(&mut b.children) {
        if part(&child) {
            flush(&mut run, &mut out);
            out.push(child);
        } else {
            run.push(child);
        }
    }
    flush(&mut run, &mut out);
    b.children = out;
}

fn wrap_inline_children(doc: &Document, b: &mut LayoutBox) {
    if !has_inflow_blocks(b) {
        return;
    }
    let anon = anonymous_style(&b.style);
    let mut out = Vec::new();
    let mut run: Vec<LayoutBox> = Vec::new();
    let flush = |run: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>| {
        if run.is_empty() {
            return;
        }
        let children = std::mem::take(run);
        if !only_whitespace(doc, &children) {
            out.push(LayoutBox::new(BoxKind::Anonymous, anon.clone(), children));
        }
    };
    for child in std::mem::take(&mut b.children) {
        if child.is_inflow_block() {
            flush(&mut run, &mut out);
            out.push(child);
        } else {
            run.push(child);
        }
    }
    flush(&mut run, &mut out);
    b.children = out;
}

/// Filhos de um contêiner flex/grid viram itens: texto solto ganha um bloco
/// anônimo e elementos em linha viram blocos ("blockification").
fn make_flex_items(doc: &Document, b: &mut LayoutBox) {
    let anon = anonymous_style(&b.style);
    let mut out = Vec::new();
    let mut run: Vec<LayoutBox> = Vec::new();
    let flush = |run: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>| {
        if run.is_empty() {
            return;
        }
        let children = std::mem::take(run);
        if !only_whitespace(doc, &children) {
            out.push(LayoutBox::new(BoxKind::Anonymous, anon.clone(), children));
        }
    };
    for child in std::mem::take(&mut b.children) {
        match child.kind {
            BoxKind::Text(_) => run.push(child),
            BoxKind::Inline(n) => {
                flush(&mut run, &mut out);
                let mut c = child;
                c.kind = BoxKind::Block(n);
                wrap_inline_children(doc, &mut c);
                out.push(c);
            }
            BoxKind::Atomic(_) => {
                flush(&mut run, &mut out);
                out.push(LayoutBox::new(BoxKind::Anonymous, anon.clone(), vec![child]));
            }
            BoxKind::InlineBlock(n) => {
                flush(&mut run, &mut out);
                let mut c = child;
                c.kind = BoxKind::Block(n);
                out.push(c);
            }
            BoxKind::Block(_) | BoxKind::Anonymous => {
                flush(&mut run, &mut out);
                out.push(child);
            }
        }
    }
    flush(&mut run, &mut out);
    b.children = out;
}

fn is_collapsible(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

// ------------------------------------------------------------------- layout

pub struct Page {
    pub items: Vec<DisplayItem>,
    pub height: f32,
    pub background: Color,
    /// Áreas clicáveis dos links.
    pub links: Vec<(Rect, String)>,
    /// Caixa (borda) de cada elemento de bloco, na ordem em que foram montadas
    /// (para cliques e `getBoundingClientRect`).
    pub boxes: Vec<(NodeId, Rect)>,
    /// Elementos que rolam por dentro (os mais internos primeiro).
    pub scrollers: Vec<Scroller>,
    /// Áreas que recebem o mouse, na ordem de pintura (a última por cima),
    /// já cortadas pelo `overflow` dos ancestrais. `true` = dentro de um
    /// `position: fixed` (coordenadas da janela, não rola).
    pub hits: Vec<(NodeId, Rect, bool)>,
    /// Elementos dentro de `position: fixed` (as caixas deles não rolam).
    pub fixed_nodes: std::collections::HashSet<NodeId>,
    /// Links dentro de `position: fixed` (coordenadas da janela).
    pub fixed_links: Vec<(Rect, String)>,
    /// Posição vertical de cada `id` (e `<a name>`), para navegar até `#âncora`.
    pub anchors: Vec<(String, f32)>,
    /// Imagens de fundo (CSS) que a página usa e ainda não foram baixadas.
    pub missing_urls: Vec<String>,
    /// Fontes web que a página usa e ainda não foram baixadas.
    pub missing_fonts: Vec<crate::css::FontFaceRule>,
}

/// Imagens já decodificadas: por elemento <img> e por URL (fundos do CSS).
#[derive(Default)]
pub struct Images {
    pub nodes: HashMap<NodeId, Arc<ImageData>>,
    pub urls: HashMap<String, Arc<ImageData>>,
}

impl Images {
    pub fn new() -> Images {
        Images::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len() + self.urls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct BoxModel {
    margin: [f32; 4],
    border: [f32; 4],
    padding: [f32; 4],
    content_w: f32,
}

impl BoxModel {
    fn anonymous(w: f32) -> Self {
        BoxModel {
            margin: [0.0; 4],
            border: [0.0; 4],
            padding: [0.0; 4],
            content_w: w,
        }
    }

    fn extra_h(&self) -> f32 {
        self.padding[LEFT] + self.padding[RIGHT] + self.border[LEFT] + self.border[RIGHT]
    }
}

/// O que um bloco passa para o conteúdo em linha dos descendentes:
/// decorações de texto e o link (`<a href>`) em que está.
#[derive(Clone, Default)]
struct Inherited {
    underline: bool,
    line_through: bool,
    link: Option<Rc<str>>,
    /// Elemento em linha mais interno (para o retângulo de cada trecho).
    elem: Option<NodeId>,
    /// O mouse atravessa (pointer-events: none ou invisível).
    no_hit: bool,
}

impl Inherited {
    fn enter(&self, doc: &Document, node: Option<NodeId>, style: &ComputedStyle) -> Inherited {
        let mut link = self.link.clone();
        if let Some(n) = node
            && let Some(el) = doc.element(n)
            && el.tag == "a"
            && let Some(href) = el.attr("href")
        {
            link = Some(Rc::from(href));
        }
        Inherited {
            underline: self.underline || style.underline,
            line_through: self.line_through || style.line_through,
            link,
            elem: None,
            no_hit: !style.pointer_events || !style.visible,
        }
    }
}

/// Caixa absoluta/fixa esperando o tamanho do contêiner posicionado.
struct Pending<'a> {
    b: &'a LayoutBox,
    /// Posição "estática": onde ela estaria no fluxo normal.
    x: f32,
    y: f32,
    inh: Inherited,
    /// Ordem na árvore (para pintar na ordem certa entre os posicionados).
    order: usize,
    /// Contextos de empilhamento em volta quando ele foi encontrado.
    ctx: Vec<(i32, usize)>,
}

/// Conteúdo posicionado ou flutuante, pintado por cima (ou por baixo, z < 0) do fluxo.
struct Layer {
    /// Onde a camada entra na ordem de pintura: o caminho de contextos de
    /// empilhamento até ela, cada passo (z-index, ordem na árvore). Ordenar
    /// pelas chaves (lexicograficamente) pinta cada contexto inteiro no seu
    /// lugar: um z-index 999 dentro de um contexto com 9999 fica dentro dele,
    /// por cima do fundo do próprio contexto. O fluxo da página é [(0, 0)].
    key: Box<[(i32, usize)]>,
    items: Vec<DisplayItem>,
    /// Dentro de um `position: fixed` (não rola com a página).
    fixed: bool,
}

/// Um float já posicionado (caixa de margem, coordenadas absolutas).
#[derive(Clone, Copy)]
struct FloatBox {
    rect: Rect,
    left: bool,
}

/// Marca de quanto já foi emitido, para mover ou apagar o que vier depois.
#[derive(Clone)]
struct Mark {
    items: usize,
    links: usize,
    boxes: usize,
    anchors: usize,
    layers: usize,
    floats: usize,
    abs: Vec<usize>,
}

/// Índices do fundo e das bordas de um bloco na lista de exibição (para
/// esticar a caixa depois: flex, grid e células de tabela).
#[derive(Clone, Copy, Default)]
struct Deco {
    /// Sombras (primeiro item, quantas).
    shadow: Option<(usize, usize)>,
    bg: Option<usize>,
    img: Option<usize>,
    border: Option<usize>,
    round_border: bool,
}

struct BlockResult {
    height: f32,
    first_baseline: Option<f32>,
    deco: Deco,
}

/// Opções de layout_block.
#[derive(Default)]
struct Opts {
    marker: Option<String>,
    forced_h: Option<f32>,
    /// Cria um novo contexto de formatação de bloco (isola floats).
    bfc: bool,
}

impl Opts {
    fn bfc() -> Opts {
        Opts {
            bfc: true,
            ..Opts::default()
        }
    }
}

pub fn layout<'a>(
    doc: &'a Document,
    root: &'a LayoutBox,
    fonts: &'a Fonts,
    images: &'a Images,
    viewport_w: f32,
    viewport_h: f32,
) -> Page {
    // O fundo do <html> (ou, se transparente, o do <body>) pinta a tela toda.
    let mut background = Color::WHITE;
    let mut skip_bg = None;
    if !root.style.background.is_transparent() {
        background = root.style.background;
        skip_bg = root.node();
    } else if let Some(body) = root
        .children
        .iter()
        .find(|c| c.node().and_then(|n| doc.tag(n)) == Some("body"))
        && !body.style.background.is_transparent()
    {
        background = body.style.background;
        skip_bg = body.node();
    }
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        w: viewport_w,
        h: viewport_h,
    };
    let mut cx = Ctx {
        doc,
        fonts,
        images,
        items: Vec::new(),
        links: Vec::new(),
        boxes: Vec::new(),
        anchors: Vec::new(),
        abs_bottom: 0.0,
        cb_heights: vec![Some(viewport_h)],
        flex_min_h: None,
        ctx_stack: Vec::new(),
        abs_order: None,
        scrollers: Vec::new(),
        in_fixed: 0,
        fixed_nodes: Default::default(),
        fixed_links: Vec::new(),
        skip_bg,
        viewport,
        abs: vec![Vec::new()],
        layers: Vec::new(),
        next_layer: 1,
        floats: Vec::new(),
        bfc_starts: vec![0],
        intrinsic_cache: HashMap::new(),
        missing: Vec::new(),
    };
    let bm = cx.box_model(&root.style, viewport_w);
    let y = bm.margin[TOP];
    let r = cx.layout_block(root, &bm, 0.0, y, &Inherited::default(), Opts::bfc());
    // absolutos sem ancestral posicionado e fixos: relativos à janela
    loop {
        let pending = std::mem::take(&mut cx.abs[0]);
        if pending.is_empty() {
            break;
        }
        for p in pending {
            cx.layout_absolute(p, viewport);
        }
    }
    let mut layers = std::mem::take(&mut cx.layers);
    // trechos marcados (relativos/sticky) viram camadas próprias
    let mut extra = Vec::new();
    let flow = split_layers(std::mem::take(&mut cx.items), false, &mut extra);
    for l in layers.iter_mut() {
        l.items = split_layers(std::mem::take(&mut l.items), l.fixed, &mut extra);
    }
    layers.extend(extra);
    layers.push(Layer { key: Box::new([(0, 0)]), items: flow, fixed: false });
    layers.sort_by(|a, b| a.key.cmp(&b.key));
    let mut items = Vec::new();
    for l in layers.iter_mut() {
        if l.fixed && !l.items.is_empty() {
            items.push(DisplayItem::FixedStart);
            items.append(&mut l.items);
            items.push(DisplayItem::FixedEnd);
        } else {
            items.append(&mut l.items);
        }
    }
    let hits = extract_hits(&mut items);
    Page {
        items,
        height: (y + r.height + bm.margin[BOTTOM]).max(viewport_h).max(cx.abs_bottom.min(200_000.0)),
        background,
        links: cx.links,
        boxes: cx.boxes,
        scrollers: cx.scrollers,
        hits,
        fixed_nodes: cx.fixed_nodes,
        fixed_links: cx.fixed_links,
        anchors: cx.anchors,
        missing_urls: cx.missing,
        missing_fonts: Vec::new(),
    }
}

/// Tira as áreas de mouse da lista de pintura, cortando cada uma pelos
/// recortes em vigor naquele ponto.
fn extract_hits(items: &mut Vec<DisplayItem>) -> Vec<(NodeId, Rect, bool)> {
    let mut saved: Vec<Option<Rect>> = Vec::new();
    let mut clip: Option<Rect> = None;
    let mut hits = Vec::new();
    let mut fixed = 0u32;
    for it in items.iter() {
        match it {
            DisplayItem::FixedStart => fixed += 1,
            DisplayItem::FixedEnd => fixed = fixed.saturating_sub(1),
            DisplayItem::PushClip(r) => {
                saved.push(clip);
                clip = Some(clip.map_or(*r, |c| c.intersect(r)));
            }
            DisplayItem::PopClip => clip = saved.pop().flatten(),
            DisplayItem::Hit { rect, node } => {
                let r = clip.map_or(*rect, |c| c.intersect(rect));
                if r.w > 0.0 && r.h > 0.0 {
                    hits.push((*node, r, fixed > 0));
                }
            }
            _ => {}
        }
    }
    if !hits.is_empty() {
        items.retain(|it| !matches!(it, DisplayItem::Hit { .. }));
    }
    hits
}

/// Tira dos itens os trechos entre LayerStart/LayerEnd (aninhados ou não),
/// devolvendo o resto; cada trecho vira uma camada em `out`.
fn split_layers(items: Vec<DisplayItem>, fixed: bool, out: &mut Vec<Layer>) -> Vec<DisplayItem> {
    if !items.iter().any(|it| matches!(it, DisplayItem::LayerStart { .. })) {
        return items
            .into_iter()
            .filter(|it| !matches!(it, DisplayItem::LayerEnd))
            .collect();
    }
    // pilha de trechos abertos: (chave, itens); a base é o resto
    let mut stack: Vec<(Option<Box<[(i32, usize)]>>, Vec<DisplayItem>)> = vec![(None, Vec::new())];
    for it in items {
        match it {
            DisplayItem::LayerStart { key } => stack.push((Some(key), Vec::new())),
            DisplayItem::LayerEnd => {
                if stack.len() > 1
                    && let Some((Some(key), items)) = stack.pop()
                {
                    out.push(Layer { key, items, fixed });
                }
            }
            other => stack.last_mut().unwrap().1.push(other),
        }
    }
    while stack.len() > 1 {
        if let Some((Some(key), items)) = stack.pop() {
            out.push(Layer { key, items, fixed });
        }
    }
    stack.pop().map(|t| t.1).unwrap_or_default()
}

struct Ctx<'a> {
    doc: &'a Document,
    fonts: &'a Fonts,
    images: &'a Images,
    items: Vec<DisplayItem>,
    links: Vec<(Rect, String)>,
    boxes: Vec<(NodeId, Rect)>,
    anchors: Vec<(String, f32)>,
    /// Fundo mais baixo dos elementos absolutos (altura rolável da página).
    abs_bottom: f32,
    /// Altura definida (do conteúdo) de cada bloco em montagem, para `height: N%`.
    cb_heights: Vec<Option<f32>>,
    /// `min-height` (do conteúdo) do contêiner flex que vai ser montado: a
    /// linha única de um flex em linha e o tamanho principal de um flex em
    /// coluna respeitam o mínimo antes de alinhar (consumido no começo).
    flex_min_h: Option<f32>,
    /// Contextos de empilhamento abertos, de fora para dentro: (z-index, ordem).
    ctx_stack: Vec<(i32, usize)>,
    /// Ordem do absoluto que está sendo montado (vale para o contexto dele).
    abs_order: Option<usize>,
    scrollers: Vec<Scroller>,
    /// Profundidade dentro de elementos `position: fixed`.
    in_fixed: u32,
    fixed_nodes: std::collections::HashSet<NodeId>,
    fixed_links: Vec<(Rect, String)>,
    skip_bg: Option<NodeId>,
    viewport: Rect,
    /// Um nível por contêiner posicionado aberto; o nível 0 é a janela.
    abs: Vec<Vec<Pending<'a>>>,
    layers: Vec<Layer>,
    next_layer: usize,
    /// Floats do contexto de formatação atual (e dos que o contêm).
    floats: Vec<FloatBox>,
    /// Onde começa, em `floats`, cada contexto de formatação aberto.
    bfc_starts: Vec<usize>,
    /// (min-content, max-content) por caixa.
    intrinsic_cache: HashMap<usize, (f32, f32)>,
    /// URLs de fundo que faltam baixar.
    missing: Vec<String>,
}

fn collapse_margins(prev: Option<f32>, next: f32) -> f32 {
    match prev {
        None => next,
        Some(p) if p >= 0.0 && next >= 0.0 => p.max(next),
        Some(p) if p < 0.0 && next < 0.0 => p.min(next),
        Some(p) => p + next,
    }
}

fn list_marker(style: ListStyle, n: i64) -> Option<String> {
    match style {
        ListStyle::None => None,
        ListStyle::Disc => Some("•  ".into()),
        ListStyle::Circle => Some("◦  ".into()),
        ListStyle::Square => Some("▪  ".into()),
        ListStyle::Decimal => Some(format!("{n}. ")),
    }
}

fn translate(it: &mut DisplayItem, dx: f32, dy: f32) {
    match it {
        DisplayItem::Rect { rect, .. }
        | DisplayItem::RoundRect { rect, .. }
        | DisplayItem::Image { rect, .. }
        | DisplayItem::Video { rect, .. }
        | DisplayItem::Canvas { rect, .. }
        | DisplayItem::Hit { rect, .. }
        | DisplayItem::Shadow { rect, .. }
        | DisplayItem::RoundBorder { rect, .. }
        | DisplayItem::Gradient { rect, .. } => {
            rect.x += dx;
            rect.y += dy;
        }
        DisplayItem::TransformStart { origin, .. } => {
            origin.0 += dx;
            origin.1 += dy;
        }
        DisplayItem::BgImage { rect, tile, .. } => {
            rect.x += dx;
            rect.y += dy;
            tile.x += dx;
            tile.y += dy;
        }
        DisplayItem::Text { x, baseline, .. } => {
            *x += dx;
            *baseline += dy;
        }
        DisplayItem::PushClip(r) => {
            r.x += dx;
            r.y += dy;
        }
        DisplayItem::PopClip
        | DisplayItem::LayerStart { .. }
        | DisplayItem::LayerEnd
        | DisplayItem::FixedStart
        | DisplayItem::FixedEnd
        | DisplayItem::TransformEnd => {}
    }
}

fn fade(it: &mut DisplayItem, opacity: f32) {
    match it {
        DisplayItem::Rect { color, .. }
        | DisplayItem::RoundRect { color, .. }
        | DisplayItem::Text { color, .. }
        | DisplayItem::Shadow { color, .. }
        | DisplayItem::RoundBorder { color, .. } => {
            color.a = (color.a as f32 * opacity).round() as u8;
        }
        _ => {}
    }
}

/// Matriz 2D `[a b c d e f]` de uma lista de funções de `transform`
/// (porcentagens de translate relativas à caixa `w`×`h`).
fn transform_matrix(fns: &[crate::style::TransformFn], w: f32, h: f32) -> [f32; 6] {
    use crate::style::TransformFn as T;
    let mul = |m: [f32; 6], n: [f32; 6]| {
        [
            m[0] * n[0] + m[2] * n[1],
            m[1] * n[0] + m[3] * n[1],
            m[0] * n[2] + m[2] * n[3],
            m[1] * n[2] + m[3] * n[3],
            m[0] * n[4] + m[2] * n[5] + m[4],
            m[1] * n[4] + m[3] * n[5] + m[5],
        ]
    };
    let mut m = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    for f in fns {
        let n = match *f {
            T::Translate(x, xp, y, yp) => [1.0, 0.0, 0.0, 1.0, x + xp / 100.0 * w, y + yp / 100.0 * h],
            T::Scale(sx, sy) => [sx, 0.0, 0.0, sy, 0.0, 0.0],
            T::Rotate(a) => [a.cos(), a.sin(), -a.sin(), a.cos(), 0.0, 0.0],
            T::Skew(ax, ay) => [1.0, ay.tan(), ax.tan(), 1.0, 0.0, 0.0],
            T::Matrix(v) => v,
        };
        m = mul(m, n);
    }
    m
}

fn px(l: Len) -> f32 {
    match l {
        Len::Px(v) => v,
        _ => 0.0,
    }
}

fn set_rect(it: &mut DisplayItem, top: Option<f32>, h: f32) {
    if let DisplayItem::Shadow { rect, .. } = it {
        // a sombra acompanha a caixa (deslocamento e espalhamento já estão nela)
        let grow = rect.h;
        let _ = grow;
        if let Some(t) = top {
            rect.y = t;
        }
        rect.h = h;
        return;
    }
    match it {
        DisplayItem::Rect { rect, .. }
        | DisplayItem::RoundRect { rect, .. }
        | DisplayItem::Gradient { rect, .. }
        | DisplayItem::RoundBorder { rect, .. }
        | DisplayItem::BgImage { rect, .. } => {
            if let Some(t) = top {
                rect.y = t;
            }
            rect.h = h;
        }
        _ => {}
    }
}

/// Raio dos cantos, resolvido contra a caixa.
fn radius_of(s: &ComputedStyle, r: Rect) -> Radii {
    let m = r.w.min(r.h).max(0.0);
    Radii(s.border_radius.map(|l| l.resolve(m).unwrap_or(0.0).max(0.0))).fit(r.w, r.h)
}

fn colspan(doc: &Document, b: &LayoutBox) -> usize {
    b.node()
        .and_then(|n| doc.element(n))
        .and_then(|e| e.attr("colspan"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(1usize)
        .clamp(1, 100)
}

/// Uma tabela organizada em linhas de células (com colspan).
struct TableGrid<'a> {
    captions: Vec<&'a LayoutBox>,
    rows: Vec<(Option<&'a LayoutBox>, Vec<(&'a LayoutBox, usize)>)>,
    ncols: usize,
}

fn table_grid<'a>(doc: &Document, b: &'a LayoutBox) -> TableGrid<'a> {
    let mut g = TableGrid {
        captions: Vec::new(),
        rows: Vec::new(),
        ncols: 0,
    };
    let cells_of = |r: &'a LayoutBox| -> Vec<(&'a LayoutBox, usize)> {
        r.children
            .iter()
            .filter(|c| c.style.display == Display::TableCell && c.is_block_level())
            .map(|c| (c, colspan(doc, c)))
            .collect()
    };
    let mut loose: Vec<(&'a LayoutBox, usize)> = Vec::new();
    let push_row = |g: &mut TableGrid<'a>, tr: Option<&'a LayoutBox>, cells: Vec<(&'a LayoutBox, usize)>| {
        if !cells.is_empty() {
            g.ncols = g.ncols.max(cells.iter().map(|c| c.1).sum());
            g.rows.push((tr, cells));
        }
    };
    for c in &b.children {
        match c.style.display {
            Display::TableCaption => g.captions.push(c),
            Display::TableRowGroup => {
                for r in &c.children {
                    match r.style.display {
                        Display::TableRow => push_row(&mut g, Some(r), cells_of(r)),
                        Display::TableCell => push_row(&mut g, None, vec![(r, colspan(doc, r))]),
                        _ => {}
                    }
                }
            }
            Display::TableRow => {
                if !loose.is_empty() {
                    push_row(&mut g, None, std::mem::take(&mut loose));
                }
                push_row(&mut g, Some(c), cells_of(c));
            }
            Display::TableCell => loose.push((c, colspan(doc, c))),
            _ => {}
        }
    }
    if !loose.is_empty() {
        push_row(&mut g, None, loose);
    }
    g
}

/// Um item flex durante o cálculo de tamanhos (eixo principal horizontal).
struct FlexItem<'a> {
    b: &'a LayoutBox,
    margin: [f32; 4],
    auto_l: bool,
    auto_r: bool,
    border: [f32; 4],
    padding: [f32; 4],
    extra: f32,
    base: f32,
    min: f32,
    max: f32,
    size: f32,
}

impl FlexItem<'_> {
    fn outer(&self) -> f32 {
        self.size + self.extra + self.margin[LEFT] + self.margin[RIGHT]
    }
}

/// Item de grid já posicionado nas trilhas.
struct GridItem<'a> {
    b: &'a LayoutBox,
    row: usize,
    col: usize,
    rspan: usize,
    cspan: usize,
}

thread_local! {
    /// Deslocamento de rolagem dos elementos roláveis (overflow auto/scroll).
    static SCROLLS: std::cell::RefCell<HashMap<NodeId, (f32, f32)>> = std::cell::RefCell::new(HashMap::new());
}

/// Diz ao layout quanto cada elemento rolável está rolado.
pub fn set_scroll_offsets(m: HashMap<NodeId, (f32, f32)>) {
    SCROLLS.with(|s| *s.borrow_mut() = m);
}

/// Um elemento que rola por dentro: caixa de preenchimento (coordenadas do
/// documento), rolagem atual e máxima.
#[derive(Clone, Copy, Debug)]
pub struct Scroller {
    pub node: NodeId,
    pub rect: Rect,
    pub x: f32,
    pub y: f32,
    pub max_x: f32,
    pub max_y: f32,
    /// Dentro de um `position: fixed` (a caixa está em coordenadas da janela).
    pub fixed: bool,
}

thread_local! {
    /// Elemento com o foco (campo de texto ganha cursor e borda de foco).
    static FOCUSED: std::cell::Cell<Option<NodeId>> = const { std::cell::Cell::new(None) };
}

/// Diz ao layout qual elemento tem o foco (a sessão da página chama antes de desenhar).
pub fn set_focus(n: Option<NodeId>) {
    FOCUSED.with(|f| f.set(n));
}

thread_local! {
    /// Tamanho natural dos `<video>` que já sabem o tamanho (largura, altura).
    static VIDEO_SIZES: std::cell::RefCell<HashMap<NodeId, (f32, f32)>> = std::cell::RefCell::new(HashMap::new());
}

/// Diz ao layout o tamanho natural de cada vídeo (a proporção da caixa segue o vídeo).
pub fn set_video_sizes(m: HashMap<NodeId, (f32, f32)>) {
    VIDEO_SIZES.with(|v| *v.borrow_mut() = m);
}

thread_local! {
    /// SVGs embutidos já desenhados: (conteúdo, tamanho, cor) → imagem.
    static INLINE_SVG: std::cell::RefCell<HashMap<(u64, u32, u32, u32), Option<Arc<ImageData>>>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Desenha um `<svg>` que está no próprio HTML (ícones, logos).
fn inline_svg(doc: &Document, n: NodeId, w: f32, h: f32, color: Color) -> Option<Arc<ImageData>> {
    use std::hash::Hasher;
    if w < 1.0 || h < 1.0 || w > 4096.0 || h > 4096.0 {
        return None;
    }
    let src = doc.serialize(n, true);
    let mut hs = ferro_js::fxhash::FxHasher::default();
    hs.write(src.as_bytes());
    // 2× para ficar nítido com zoom e telas de alta densidade
    let (pw, ph) = ((w * 2.0).round() as u32, (h * 2.0).round() as u32);
    let key = (hs.finish(), pw, ph, u32::from_le_bytes([color.r, color.g, color.b, color.a]));
    INLINE_SVG.with(|c| {
        if let Some(v) = c.borrow().get(&key) {
            return v.clone();
        }
        let img = crate::svg::parse(&src).map(|svg| Arc::new(svg.render_with(pw, ph, None, color)));
        let mut c = c.borrow_mut();
        if c.len() > 2000 {
            c.clear();
        }
        c.insert(key, img.clone());
        img
    })
}

fn track_fixed(t: Track, w: f32) -> Option<f32> {
    match t {
        Track::Len(l) => l.resolve(w),
        _ => None,
    }
}

impl<'a> Ctx<'a> {
    // ---------------------------------------------------- marcas e camadas

    fn mark(&self) -> Mark {
        Mark {
            items: self.items.len(),
            links: self.links.len(),
            boxes: self.boxes.len(),
            anchors: self.anchors.len(),
            layers: self.layers.len(),
            floats: self.floats.len(),
            abs: self.abs.iter().map(Vec::len).collect(),
        }
    }

    /// Move tudo o que foi emitido entre as marcas `a` e `b` (b = None: até agora).
    fn shift_between(&mut self, a: &Mark, b: Option<&Mark>, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let end = |len: usize, f: fn(&Mark) -> usize| b.map_or(len, f);
        let ie = end(self.items.len(), |m| m.items);
        for it in &mut self.items[a.items..ie] {
            translate(it, dx, dy);
        }
        let le = end(self.links.len(), |m| m.links);
        for (r, _) in &mut self.links[a.links..le] {
            r.x += dx;
            r.y += dy;
        }
        let be = end(self.boxes.len(), |m| m.boxes);
        for (_, r) in &mut self.boxes[a.boxes.min(be)..be] {
            r.x += dx;
            r.y += dy;
        }
        let ae = end(self.anchors.len(), |m| m.anchors);
        for (_, y) in &mut self.anchors[a.anchors..ae] {
            *y += dy;
        }
        let ye = end(self.layers.len(), |m| m.layers);
        for l in &mut self.layers[a.layers..ye] {
            for it in &mut l.items {
                translate(it, dx, dy);
            }
        }
        let fe = end(self.floats.len(), |m| m.floats).min(self.floats.len());
        for f in &mut self.floats[a.floats.min(fe)..fe] {
            f.rect.x += dx;
            f.rect.y += dy;
        }
        for (lvl, &start) in a.abs.iter().enumerate() {
            let stop = b.and_then(|m| m.abs.get(lvl).copied());
            if let Some(v) = self.abs.get_mut(lvl) {
                let stop = stop.unwrap_or(v.len()).min(v.len());
                for p in &mut v[start.min(stop)..stop] {
                    p.x += dx;
                    p.y += dy;
                }
            }
        }
    }

    fn fade_since(&mut self, m: &Mark, opacity: f32) {
        if opacity <= 0.01 {
            // invisível, mas continua recebendo o mouse
            let tail = self.items.split_off(m.items);
            self.items.extend(tail.into_iter().filter(|it| {
                matches!(
                    it,
                    DisplayItem::Hit { .. }
                        | DisplayItem::PushClip(_)
                        | DisplayItem::PopClip
                        | DisplayItem::LayerStart { .. }
                        | DisplayItem::LayerEnd
                )
            }));
            self.layers.truncate(m.layers);
            return;
        }
        for it in &mut self.items[m.items..] {
            fade(it, opacity);
        }
        for l in &mut self.layers[m.layers..] {
            for it in &mut l.items {
                fade(it, opacity);
            }
        }
    }

    /// Coloca o fundo e as bordas da caixa em `top` com altura `h`.
    fn stretch_to(&mut self, d: Deco, top: Option<f32>, h: f32) {
        let len = self.items.len();
        let ok = |i: &usize| *i < len;
        let d = Deco {
            shadow: d.shadow.filter(|(i, n)| i + n <= len),
            bg: d.bg.filter(ok),
            img: d.img.filter(ok),
            border: d.border.filter(|i| i + if d.round_border { 0 } else { 3 } < len),
            round_border: d.round_border,
        };
        for i in [d.bg, d.img].into_iter().flatten() {
            set_rect(&mut self.items[i], top, h);
        }
        if let Some((i, n)) = d.shadow {
            for k in i..i + n {
                set_rect(&mut self.items[k], top, h);
            }
        }
        if let Some(i) = d.border
            && d.round_border
        {
            set_rect(&mut self.items[i], top, h);
            return;
        }
        if let Some(i) = d.border {
            let t = match (&self.items[i], top) {
                (_, Some(t)) => t,
                (DisplayItem::Rect { rect, .. }, None) => rect.y,
                _ => return,
            };
            for k in 0..4 {
                if let DisplayItem::Rect { rect, .. } = &mut self.items[i + k] {
                    match k {
                        0 => rect.y = t,
                        2 => rect.y = t + h - rect.h,
                        _ => {
                            rect.y = t;
                            rect.h = h;
                        }
                    }
                }
            }
        }
    }

    fn stretch(&mut self, d: Deco, h: f32) {
        self.stretch_to(d, None, h);
    }

    /// Chave de pintura de uma camada nova (ver `Layer`). Com z-index, o
    /// elemento é um contexto: o conteúdo próprio dele vai em (0, 0) dentro do
    /// contexto, antes dos filhos com z >= 0; sem z-index, entra no contexto
    /// em volta na ordem da árvore.
    fn layer_key(&self, z: Option<i32>, order: usize) -> Box<[(i32, usize)]> {
        let mut k = self.ctx_stack.clone();
        match z {
            Some(z) => {
                k.push((z, order));
                k.push((0, 0));
            }
            None => k.push((0, order)),
        }
        k.into_boxed_slice()
    }

    fn add_pending(&mut self, b: &'a LayoutBox, x: f32, y: f32, inh: &Inherited) {
        let order = self.next_layer;
        self.next_layer += 1;
        let p = Pending {
            b,
            x,
            y,
            inh: inh.clone(),
            order,
            ctx: self.ctx_stack.clone(),
        };
        if b.style.position == Position::Fixed {
            self.abs[0].push(p);
        } else {
            self.abs.last_mut().unwrap().push(p);
        }
    }

    // -------------------------------------------------------------- fundos

    /// A imagem dessa URL já chegou? (Registra o que falta baixar.)
    fn url_ready(&mut self, u: &str) -> bool {
        if self.images.urls.contains_key(u) {
            return true;
        }
        if !self.missing.iter().any(|m| m == u) && self.missing.len() < 150 {
            self.missing.push(u.to_string());
        }
        false
    }

    /// O fundo-imagem/gradiente pode ser pintado agora?
    fn bg_ready(&mut self, s: &ComputedStyle) -> bool {
        match &s.bg_image {
            None => false,
            Some(BgImage::Linear(_)) => true,
            Some(BgImage::Url(u)) => self.url_ready(u),
        }
    }

    fn bg_item(&self, s: &ComputedStyle, r: Rect, radius: Radii) -> Option<DisplayItem> {
        if let Some(mask) = &s.mask_image {
            let img = self.images.urls.get(&**mask)?;
            let tile = self.tile_rect(img, r, s.mask_size, s.mask_pos);
            return Some(DisplayItem::BgImage {
                rect: r,
                image: img.clone(),
                tile,
                repeat: s.mask_repeat,
                tint: Some(s.background),
            });
        }
        match s.bg_image.as_ref()? {
            BgImage::Linear(g) => Some(DisplayItem::Gradient {
                rect: r,
                radius,
                angle: g.angle,
                stops: Arc::new(g.stops.clone()),
            }),
            BgImage::Url(u) => {
                let img = self.images.urls.get(&**u)?;
                Some(DisplayItem::BgImage {
                    rect: r,
                    image: img.clone(),
                    tile: self.tile_rect(img, r, s.bg_size, s.bg_pos),
                    repeat: s.bg_repeat,
                    tint: None,
                })
            }
        }
    }

    /// Posição e tamanho de uma cópia da imagem de fundo dentro de `r`.
    fn tile_rect(&self, img: &ImageData, r: Rect, size: BgSize, at: (Len, Len)) -> Rect {
        {
            {
                let (iw, ih) = (img.width.max(1) as f32, img.height.max(1) as f32);
                let (tw, th) = match size {
                    BgSize::Auto => (iw, ih),
                    BgSize::Cover => {
                        let k = (r.w / iw).max(r.h / ih);
                        (iw * k, ih * k)
                    }
                    BgSize::Contain => {
                        let k = (r.w / iw).min(r.h / ih);
                        (iw * k, ih * k)
                    }
                    BgSize::Size(w, h) => match (w.resolve(r.w), h.resolve(r.h)) {
                        (Some(w), Some(h)) => (w, h),
                        (Some(w), None) => (w, w * ih / iw),
                        (None, Some(h)) => (h * iw / ih, h),
                        _ => (iw, ih),
                    },
                };
                let pos = |l: Len, free: f32| l.resolve(free).unwrap_or(0.0);
                Rect {
                    x: r.x + pos(at.0, r.w - tw),
                    y: r.y + pos(at.1, r.h - th),
                    w: tw,
                    h: th,
                }
            }
        }
    }

    /// Fundo e bordas de um trecho de elemento em linha.
    fn paint_inline_seg(&mut self, seg: &Seg, line_x: f32, baseline: f32) {
        let s = &seg.style;
        if !s.visible {
            return;
        }
        let font = self.fonts.for_style(s);
        let m = self.fonts.line_metrics(font, s.font_size);
        let (bt, bb) = (s.border(TOP), s.border(BOTTOM));
        let bl = if seg.first { s.border(LEFT) } else { 0.0 };
        let br = if seg.last { s.border(RIGHT) } else { 0.0 };
        let (pt, pb) = (px(s.padding[TOP]), px(s.padding[BOTTOM]));
        let r = Rect {
            x: line_x + seg.x0,
            y: baseline - m.ascent - pt - bt,
            w: (seg.x1 - seg.x0).max(0.0),
            h: m.ascent + m.descent + pt + pb + bt + bb,
        };
        self.paint_decor(s, r, [bt, br, bb, bl]);
    }

    /// Fundo (cor e degradê) e bordas de `r`; `b` são as larguras das bordas
    /// (topo, direita, baixo, esquerda).
    fn paint_decor(&mut self, s: &ComputedStyle, r: Rect, b: [f32; 4]) {
        let [bt, br, bb, bl] = b;
        let radius = radius_of(s, r);
        if !s.background.is_transparent() && s.mask_image.is_none() {
            self.items.push(if !radius.is_zero() {
                DisplayItem::RoundRect {
                    rect: r,
                    radius,
                    color: s.background,
                }
            } else {
                DisplayItem::Rect {
                    rect: r,
                    color: s.background,
                }
            });
        }
        if let Some(BgImage::Linear(_)) = &s.bg_image
            && let Some(item) = self.bg_item(s, r, radius)
        {
            self.items.push(item);
        }
        if bt + bb + bl + br > 0.0 {
            if !radius.is_zero() {
                self.items.push(DisplayItem::RoundBorder {
                    rect: r,
                    radius,
                    widths: [bt, br, bb, bl],
                    color: s.border_color(TOP),
                });
            } else {
                for (k, rect) in [
                    Rect { x: r.x, y: r.y, w: r.w, h: bt },
                    Rect { x: r.x + r.w - br, y: r.y, w: br, h: r.h },
                    Rect { x: r.x, y: r.y + r.h - bb, w: r.w, h: bb },
                    Rect { x: r.x, y: r.y, w: bl, h: r.h },
                ]
                .into_iter()
                .enumerate()
                {
                    if rect.w > 0.0 && rect.h > 0.0 {
                        self.items.push(DisplayItem::Rect {
                            rect,
                            color: s.border_color(k),
                        });
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------- floats

    fn bfc_floats(&self) -> &[FloatBox] {
        let start = self.bfc_starts.last().copied().unwrap_or(0).min(self.floats.len());
        &self.floats[start..]
    }

    /// Altura abaixo da qual não há floats do lado pedido.
    fn clear_y(&self, clear: Clear) -> f32 {
        self.bfc_floats()
            .iter()
            .filter(|f| match clear {
                Clear::Left => f.left,
                Clear::Right => !f.left,
                Clear::Both => true,
                Clear::None => false,
            })
            .map(|f| f.rect.y + f.rect.h)
            .fold(f32::MIN, f32::max)
    }

    /// Faixa livre [esquerda, direita) na altura `y`, dentro de [x, x + w).
    fn free_band(&self, y: f32, h: f32, x: f32, w: f32) -> (f32, f32, Option<f32>) {
        let (mut l, mut r) = (x, x + w);
        let mut next: Option<f32> = None;
        for f in self.bfc_floats() {
            if f.rect.y < y + h && f.rect.y + f.rect.h > y {
                if f.left {
                    l = l.max(f.rect.x + f.rect.w);
                } else {
                    r = r.min(f.rect.x);
                }
                let b = f.rect.y + f.rect.h;
                next = Some(next.map_or(b, |n| n.min(b)));
            }
        }
        (l, r, next)
    }

    /// Posiciona um float no bloco contêiner [cb_x, cb_x + cb_w) a partir de `y`.
    fn place_float(&mut self, b: &'a LayoutBox, cb_x: f32, cb_w: f32, y: f32, inh: &Inherited) -> FloatBox {
        let s = &b.style;
        let left = s.float != Float::Right;
        let bm = self.shrink_bm(b, cb_w, false);
        let outer_w = bm.content_w + bm.extra_h() + bm.margin[LEFT] + bm.margin[RIGHT];
        let mut top = y;
        if s.clear != Clear::None {
            top = top.max(self.clear_y(s.clear));
        }
        // floats anteriores ficam acima ou ao lado, nunca abaixo
        if let Some(prev) = self.bfc_floats().last() {
            top = top.max(prev.rect.y);
        }
        let mut guard = 0;
        let (l, r) = loop {
            let (l, r, next) = self.free_band(top, 1.0, cb_x, cb_w);
            match next {
                Some(n) if r - l < outer_w - 0.5 && guard < 50 => {
                    top = n;
                    guard += 1;
                }
                _ => break (l, r),
            }
        };
        let x = if left { l } else { r - outer_w };
        let order = self.next_layer;
        self.next_layer += 1;
        let mark = self.mark();
        let res = self.layout_block(b, &bm, x, top + bm.margin[TOP], inh, Opts::bfc());
        let items = self.items.split_off(mark.items);
        self.layers.push(Layer { key: self.layer_key(None, order), items, fixed: self.in_fixed > 0 });
        let f = FloatBox {
            rect: Rect {
                x,
                y: top,
                w: outer_w,
                h: bm.margin[TOP] + res.height + bm.margin[BOTTOM],
            },
            left,
        };
        self.floats.push(f);
        f
    }

    // ------------------------------------------------------------ blocos

    /// Larguras e margens de uma caixa de bloco (CSS 2.1 §10.3.3).
    fn box_model(&self, s: &ComputedStyle, cb_w: f32) -> BoxModel {
        let border = [s.border(TOP), s.border(RIGHT), s.border(BOTTOM), s.border(LEFT)];
        let padding = s.padding.map(|p| p.resolve_or_zero(cb_w).max(0.0));
        let extra = padding[LEFT] + padding[RIGHT] + border[LEFT] + border[RIGHT];
        let content = |w: f32| if s.border_box { (w - extra).max(0.0) } else { w };
        let mt = s.margin[TOP].resolve_or_zero(cb_w);
        let mb = s.margin[BOTTOM].resolve_or_zero(cb_w);
        let ml = s.margin[LEFT].resolve(cb_w);
        let mr = s.margin[RIGHT].resolve(cb_w);
        let max_w = s.max_width.resolve(cb_w).map(content);
        let min_w = s.min_width.resolve(cb_w).map(content);

        let width = match s.width.resolve(cb_w).map(content) {
            Some(w) => w,
            None => {
                let auto_w = (cb_w - ml.unwrap_or(0.0) - mr.unwrap_or(0.0) - extra).max(0.0);
                match max_w {
                    // largura automática limitada por max-width: aí as margens auto centralizam
                    Some(m) if auto_w > m => m,
                    _ => {
                        return BoxModel {
                            margin: [mt, mr.unwrap_or(0.0), mb, ml.unwrap_or(0.0)],
                            border,
                            padding,
                            content_w: min_w.map_or(auto_w, |m| auto_w.max(m)),
                        };
                    }
                }
            }
        };
        let width = max_w.map_or(width, |m| width.min(m));
        let width = min_w.map_or(width, |m| width.max(m));
        let remaining = cb_w - width - extra;
        let (ml, mr) = match (ml, mr) {
            (None, None) if remaining > 0.0 => (remaining / 2.0, remaining / 2.0),
            (None, None) => (0.0, remaining),
            (None, Some(r)) => (remaining - r, r),
            (Some(l), None) => (l, remaining - l),
            (Some(l), Some(r)) => (l, r),
        };
        BoxModel {
            margin: [mt, mr, mb, ml],
            border,
            padding,
            content_w: width,
        }
    }

    /// Como box_model, mas com largura automática "encolhe para caber"
    /// (tabelas e floats). `center_auto` mantém margens auto centralizando.
    fn shrink_bm(&mut self, b: &'a LayoutBox, cb_w: f32, center_auto: bool) -> BoxModel {
        let mut bm = self.box_model(&b.style, cb_w);
        let s = &b.style;
        if s.width != Len::Auto {
            if !center_auto {
                bm.margin[LEFT] = s.margin[LEFT].resolve_or_zero(cb_w);
                bm.margin[RIGHT] = s.margin[RIGHT].resolve_or_zero(cb_w);
            }
            return bm;
        }
        let extra = bm.extra_h();
        let (mn, mx) = self.intrinsic(b);
        let fill = bm.content_w;
        let w = match s.width_kw {
            Some(SizeKw::MaxContent) => (mx - extra).max(0.0),
            Some(SizeKw::MinContent) => (mn - extra).max(0.0),
            _ => (mx - extra).min(fill).max((mn - extra).max(0.0)),
        };
        bm.content_w = w;
        let ml = s.margin[LEFT].resolve(cb_w);
        let mr = s.margin[RIGHT].resolve(cb_w);
        let free = cb_w - w - extra;
        let (l, r) = match (ml, mr, center_auto) {
            (None, None, true) => ((free / 2.0).max(0.0), (free / 2.0).max(0.0)),
            (None, Some(r), true) => ((free - r).max(0.0), r),
            (l, r, _) => (l.unwrap_or(0.0), r.unwrap_or(0.0)),
        };
        bm.margin[LEFT] = l;
        bm.margin[RIGHT] = r;
        bm
    }

    fn layout_block(&mut self, b: &'a LayoutBox, bm: &BoxModel, x: f32, y: f32, inh: &Inherited, opts: Opts) -> BlockResult {
        let s = &b.style;
        let anon = matches!(b.kind, BoxKind::Anonymous);
        let inh = inh.enter(self.doc, b.node(), s);
        if !b.generated
            && let Some(id) = b.node().and_then(|n| self.doc.element(n)).and_then(|e| e.id())
        {
            self.anchors.push((id.to_string(), y));
        }
        let positioned = s.is_positioned() && !anon;
        if positioned {
            self.abs.push(Vec::new());
        }
        let bfc = opts.bfc
            || (!anon
                && (s.overflow_clip
                    || s.display.is_flex()
                    || s.display.is_grid()
                    || s.display.is_inline_block()
                    || matches!(s.display, Display::Table | Display::TableCell)));
        if bfc {
            self.bfc_starts.push(self.floats.len());
        }
        let needs_mark =
            s.opacity < 1.0 || s.translate.is_some() || matches!(s.position, Position::Relative | Position::Sticky);
        // relativos e sticky pintam na ordem da árvore junto com os absolutos
        let layered = !anon && matches!(s.position, Position::Relative | Position::Sticky);
        // contexto de empilhamento: posicionado com z-index (os descendentes
        // posicionados sem z-index pintam dentro dele)
        let stacking = !anon && s.is_positioned() && s.z_index.is_some();
        // ordem na árvore: a do absoluto (dada quando foi encontrado) ou nova
        let abs_order = self.abs_order.take();
        let layer_order = if layered {
            let order = self.next_layer;
            self.next_layer += 1;
            self.items.push(DisplayItem::LayerStart { key: self.layer_key(s.z_index, order) });
            Some(order)
        } else {
            abs_order
        };
        if stacking {
            self.ctx_stack.push((s.z_index.unwrap_or(0), layer_order.unwrap_or(0)));
        }
        let mark = needs_mark.then(|| self.mark());
        // transformação (rotate/scale...): marca reservada aqui, preenchida no fim
        let xform_idx = (s.transform.is_some() && !anon).then(|| {
            self.items.push(DisplayItem::TransformStart { m: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], origin: (0.0, 0.0) });
            self.items.len() - 1
        });
        // área de mouse: reservada aqui (ordem de pintura), preenchida no fim
        let hit_idx = match b.node() {
            Some(n) if !anon && !b.generated && s.pointer_events && s.visible => {
                self.items.push(DisplayItem::Hit { rect: Rect::default(), node: n });
                Some(self.items.len() - 1)
            }
            _ => None,
        };

        let bx = x + bm.margin[LEFT];
        let content_x = bx + bm.border[LEFT] + bm.padding[LEFT];
        let content_y = y + bm.border[TOP] + bm.padding[TOP];

        // Fundo, bordas e recorte entram antes do conteúdo; o tamanho final só
        // é conhecido depois, então reservamos as posições e preenchemos no fim.
        let mut deco = Deco::default();
        let canvas_bg = b.node().is_some() && b.node() == self.skip_bg && !b.generated;
        let masked = s.mask_image.is_some();
        let paint_bg = s.visible && !s.background.is_transparent() && !canvas_bg && !masked;
        if s.visible
            && !anon
            && let Some(list) = &s.box_shadow
        {
            // a última sombra da lista fica por baixo
            let n = list.len().min(4);
            deco.shadow = Some((self.items.len(), n));
            for _ in 0..n {
                self.items.push(DisplayItem::Shadow { rect: Rect::default(), radius: Radii::ZERO, blur: 0.0, color: Color::TRANSPARENT });
            }
        }
        if paint_bg {
            deco.bg = Some(self.items.len());
            self.items.push(DisplayItem::Rect {
                rect: Rect::default(),
                color: s.background,
            });
        }
        let img_ready = if masked {
            let u = s.mask_image.clone().unwrap();
            !s.background.is_transparent() && self.url_ready(&u)
        } else {
            self.bg_ready(s)
        };
        if s.visible && img_ready {
            deco.img = Some(self.items.len());
            self.items.push(DisplayItem::Rect {
                rect: Rect::default(),
                color: Color::TRANSPARENT,
            });
        }
        let has_border = s.visible && bm.border.iter().any(|&w| w > 0.0);
        deco.round_border = has_border && s.border_radius.iter().any(|l| *l != Len::Px(0.0));
        if has_border {
            deco.border = Some(self.items.len());
            for _ in 0..if deco.round_border { 1 } else { 4 } {
                self.items.push(DisplayItem::Rect {
                    rect: Rect::default(),
                    color: Color::TRANSPARENT,
                });
            }
        }
        let clip_idx = self.items.len();
        let clip = s.overflow_clip || s.line_clamp.is_some();
        if clip {
            self.items.push(DisplayItem::PushClip(Rect::default()));
        }
        // o que vem daqui em diante é conteúdo (rola junto quando o elemento rola)
        let content_mark = self.mark();

        let vert_extra = bm.border[TOP] + bm.border[BOTTOM] + bm.padding[TOP] + bm.padding[BOTTOM];
        // porcentagens de altura valem quando o pai tem altura definida
        let parent_h = self.cb_heights.last().copied().flatten();
        let fixed = |l: Len| {
            let v = match l {
                Len::Px(v) => v,
                Len::Percent(_) | Len::Calc(..) => l.resolve(parent_h?)?,
                Len::Auto => return None,
            };
            Some(if s.border_box { (v - vert_extra).max(0.0) } else { v })
        };
        let definite_h = opts.forced_h.or_else(|| fixed(s.height));
        self.cb_heights.push(definite_h);

        let w = bm.content_w;
        let (mut content_h, mut first_baseline) = match s.display {
            _ if anon => {
                if has_inflow_blocks(b) {
                    self.layout_block_children(b, content_x, content_y, w, &inh)
                } else {
                    self.layout_inline(b, content_x, content_y, w, &inh)
                }
            }
            d if d.is_flex() => {
                self.flex_min_h = if definite_h.is_none() { fixed(s.min_height) } else { None };
                self.layout_flex(b, content_x, content_y, w, definite_h, &inh)
            }
            d if d.is_grid() => self.layout_grid(b, content_x, content_y, w, definite_h, &inh),
            Display::Table => self.layout_table(b, content_x, content_y, w, &inh),
            _ if has_inflow_blocks(b) => self.layout_block_children(b, content_x, content_y, w, &inh),
            _ => self.layout_inline(b, content_x, content_y, w, &inh),
        };
        self.cb_heights.pop();

        if bfc {
            // um contexto de formatação novo contém os seus floats
            let start = self.bfc_starts.pop().unwrap_or(0).min(self.floats.len());
            let bottom = self.floats[start..].iter().map(|f| f.rect.y + f.rect.h).fold(content_y, f32::max);
            content_h = content_h.max(bottom - content_y);
            self.floats.truncate(start);
        }

        let mut h = match (definite_h, s.aspect_ratio) {
            (Some(h), _) => h,
            // aspect-ratio: a altura vem da largura (o conteúdo pode aumentar)
            (None, Some(r)) => content_h.max(w / r),
            (None, None) => content_h,
        };
        if let Some(max) = fixed(s.max_height) {
            h = h.min(max);
        }
        if let Some(n) = s.line_clamp {
            let lh = match s.line_height {
                crate::style::LineHeight::Normal => s.font_size * 1.2,
                crate::style::LineHeight::Number(k) => s.font_size * k,
                crate::style::LineHeight::Px(p) => p,
            };
            h = h.min(n as f32 * lh);
        }
        if let Some(min) = fixed(s.min_height) {
            h = h.max(min);
        }
        // <button> mais alto que o conteúdo: o conteúdo fica no meio (como no Chrome)
        if h > content_h + 0.5
            && !anon
            && !s.display.is_flex()
            && !s.display.is_grid()
            && b.node().and_then(|n| self.doc.element(n)).is_some_and(|e| e.tag == "button")
        {
            let dy = (h - content_h) / 2.0;
            self.shift_between(&content_mark, None, 0.0, dy);
            first_baseline = first_baseline.map(|b| b + dy);
        }
        let border_box = Rect {
            x: bx,
            y,
            w: bm.content_w + bm.padding[LEFT] + bm.padding[RIGHT] + bm.border[LEFT] + bm.border[RIGHT],
            h: h + vert_extra,
        };

        let radius = radius_of(s, border_box);
        if let Some((i, n)) = deco.shadow
            && let Some(list) = &s.box_shadow
        {
            for (k, sh) in list.iter().take(n).rev().enumerate() {
                if let Some(slot) = self.items.get_mut(i + k) {
                    *slot = DisplayItem::Shadow {
                        rect: Rect {
                            x: border_box.x + sh.x - sh.spread,
                            y: border_box.y + sh.y - sh.spread,
                            w: (border_box.w + 2.0 * sh.spread).max(0.0),
                            h: (border_box.h + 2.0 * sh.spread).max(0.0),
                        },
                        // o raio cresce com o espalhamento; canto reto continua reto
                        radius: Radii(radius.0.map(|r| if r > 0.0 { (r + sh.spread).max(0.0) } else { 0.0 })),
                        blur: sh.blur,
                        color: sh.color,
                    };
                }
            }
        }
        if let Some(i) = deco.bg {
            self.items[i] = if !radius.is_zero() {
                DisplayItem::RoundRect {
                    rect: border_box,
                    radius,
                    color: s.background,
                }
            } else {
                DisplayItem::Rect {
                    rect: border_box,
                    color: s.background,
                }
            };
        }
        if let Some(i) = deco.img
            && let Some(item) = self.bg_item(s, border_box, radius)
        {
            self.items[i] = item;
        }
        if let Some(i) = deco.border
            && deco.round_border
        {
            self.items[i] = DisplayItem::RoundBorder {
                rect: border_box,
                radius,
                widths: bm.border,
                color: s.border_color(TOP),
            };
        } else if let Some(i) = deco.border {
            let r = border_box;
            let bw = bm.border;
            let rects = [
                Rect { x: r.x, y: r.y, w: r.w, h: bw[TOP] },
                Rect { x: r.x + r.w - bw[RIGHT], y: r.y, w: bw[RIGHT], h: r.h },
                Rect { x: r.x, y: r.y + r.h - bw[BOTTOM], w: r.w, h: bw[BOTTOM] },
                Rect { x: r.x, y: r.y, w: bw[LEFT], h: r.h },
            ];
            for (k, rect) in rects.into_iter().enumerate() {
                self.items[i + k] = DisplayItem::Rect {
                    rect,
                    color: s.border_color(k),
                };
            }
        }
        // rolagem por dentro: mede o conteúdo e desloca o que foi rolado
        if (s.scroll_x || s.scroll_y)
            && !anon
            && !b.generated
            && let Some(n) = b.node()
        {
            let pad_box = Rect {
                x: bx + bm.border[LEFT],
                y: y + bm.border[TOP],
                w: border_box.w - bm.border[LEFT] - bm.border[RIGHT],
                h: border_box.h - bm.border[TOP] - bm.border[BOTTOM],
            };
            let right = self.boxes[content_mark.boxes.min(self.boxes.len())..]
                .iter()
                .map(|(_, r)| r.x + r.w)
                .fold(pad_box.x + pad_box.w, f32::max);
            let bottom = (content_y + content_h + bm.padding[BOTTOM]).max(pad_box.y + pad_box.h);
            let max_x = if s.scroll_x { (right + bm.padding[RIGHT] - (pad_box.x + pad_box.w)).max(0.0) } else { 0.0 };
            let max_y = if s.scroll_y { (bottom - (pad_box.y + pad_box.h)).max(0.0) } else { 0.0 };
            let (ox, oy) = SCROLLS.with(|m| m.borrow().get(&n).copied()).unwrap_or((0.0, 0.0));
            let (ox, oy) = (ox.clamp(0.0, max_x), oy.clamp(0.0, max_y));
            if ox != 0.0 || oy != 0.0 {
                self.shift_between(&content_mark, None, -ox, -oy);
            }
            self.scrollers.push(Scroller { node: n, rect: pad_box, x: ox, y: oy, max_x, max_y, fixed: self.in_fixed > 0 });
        }
        if clip {
            self.items[clip_idx] = DisplayItem::PushClip(Rect {
                x: bx + bm.border[LEFT],
                y: y + bm.border[TOP],
                w: border_box.w - bm.border[LEFT] - bm.border[RIGHT],
                h: border_box.h - bm.border[TOP] - bm.border[BOTTOM],
            });
            self.items.push(DisplayItem::PopClip);
        }

        if let Some(m) = opts.marker
            && s.visible
        {
            let font = self.fonts.for_style(s);
            let mw = self.fonts.measure(font, s.font_size, &m);
            let baseline =
                first_baseline.unwrap_or_else(|| content_y + self.fonts.line_metrics(font, s.font_size).ascent);
            self.items.push(DisplayItem::Text {
                x: content_x - mw,
                baseline,
                text: m,
                font,
                size: s.font_size,
                color: s.color,
            });
        }

        if !anon
            && !b.generated
            && let Some(n) = b.node()
        {
            let r = Rect {
                x: bx,
                y,
                w: border_box.w,
                h: border_box.h,
            };
            self.boxes.push((n, r));
            if self.in_fixed > 0 {
                self.fixed_nodes.insert(n);
            }
            if let Some(i) = hit_idx
                && let Some(DisplayItem::Hit { rect, node }) = self.items.get_mut(i)
                && *node == n
            {
                *rect = r;
            }
        }

        // absolutos que dependem deste bloco: agora o tamanho dele é conhecido
        if positioned {
            let pending = self.abs.pop().unwrap_or_default();
            let pad_box = Rect {
                x: bx + bm.border[LEFT],
                y: y + bm.border[TOP],
                w: border_box.w - bm.border[LEFT] - bm.border[RIGHT],
                h: border_box.h - bm.border[TOP] - bm.border[BOTTOM],
            };
            for p in pending {
                self.layout_absolute(p, pad_box);
            }
        }

        if let Some(m) = mark {
            if matches!(s.position, Position::Relative | Position::Sticky) {
                let dx = match (s.left, s.right) {
                    (Len::Px(l), _) => l,
                    (_, Len::Px(r)) => -r,
                    _ => 0.0,
                };
                let dy = match (s.top, s.bottom) {
                    (Len::Px(t), _) => t,
                    (_, Len::Px(b)) => -b,
                    _ => 0.0,
                };
                self.shift_between(&m, None, dx, dy);
            }
            if let Some(t) = s.translate {
                let dx = t[0] + t[1] / 100.0 * border_box.w;
                let dy = t[2] + t[3] / 100.0 * border_box.h;
                self.shift_between(&m, None, dx, dy);
            }
            if s.opacity < 1.0 {
                self.fade_since(&m, s.opacity);
                if s.opacity <= 0.01 {
                    // os itens (fundo e bordas inclusive) foram descartados
                    deco = Deco::default();
                }
            }
        }
        // fecha a transformação: matriz em px, em torno do transform-origin
        if let Some(i) = xform_idx
            && let Some(fns) = &s.transform
        {
            let bb = self.boxes.iter().rev().find(|(n, _)| Some(*n) == b.node()).map(|(_, r)| *r).unwrap_or(border_box);
            let m = transform_matrix(fns, bb.w, bb.h);
            let ox = bb.x + s.transform_origin.0.resolve(bb.w).unwrap_or(bb.w / 2.0);
            let oy = bb.y + s.transform_origin.1.resolve(bb.h).unwrap_or(bb.h / 2.0);
            if let Some(DisplayItem::TransformStart { m: mm, origin }) = self.items.get_mut(i) {
                *mm = m;
                *origin = (ox, oy);
                self.items.push(DisplayItem::TransformEnd);
            }
        }
        if stacking {
            self.ctx_stack.pop();
        }
        if layered {
            self.items.push(DisplayItem::LayerEnd);
        }

        BlockResult {
            height: border_box.h,
            first_baseline,
            deco,
        }
    }

    fn layout_block_children(
        &mut self,
        b: &'a LayoutBox,
        x: f32,
        y: f32,
        w: f32,
        inh: &Inherited,
    ) -> (f32, Option<f32>) {
        let mut cursor = y;
        let mut prev_mb: Option<f32> = None;
        let mut first_baseline = None;
        let mut counter: i64 = b
            .node()
            .and_then(|n| self.doc.element(n))
            .and_then(|e| e.attr("start"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(1);
        for child in &b.children {
            if child.is_out_of_flow() {
                self.add_pending(child, x, cursor, inh);
                continue;
            }
            let mut bm = match child.kind {
                BoxKind::Anonymous => BoxModel::anonymous(w),
                _ if child.style.display == Display::Table || child.style.width_kw.is_some() => {
                    self.shrink_bm(child, w, true)
                }
                _ => self.box_model(&child.style, w),
            };
            // text-align: -webkit-center (<center>): o bloco mais estreito que
            // o pai vai para o meio, se as margens não forem auto
            if b.style.text_align == TextAlign::WebkitCenter
                && !matches!(child.kind, BoxKind::Anonymous)
                && child.style.margin[LEFT] != Len::Auto
                && child.style.margin[RIGHT] != Len::Auto
            {
                let free = w - bm.content_w - bm.extra_h() - bm.margin[LEFT] - bm.margin[RIGHT];
                if free > 0.5 {
                    bm.margin[LEFT] += free / 2.0;
                    bm.margin[RIGHT] += free / 2.0;
                }
            }
            let mut top = cursor + collapse_margins(prev_mb, bm.margin[TOP]);
            if child.style.clear != Clear::None {
                top = top.max(self.clear_y(child.style.clear));
            }
            let marker = if child.style.display == Display::ListItem {
                if let Some(v) = child
                    .node()
                    .and_then(|n| self.doc.element(n))
                    .and_then(|e| e.attr("value"))
                    .and_then(|v| v.trim().parse().ok())
                {
                    counter = v;
                }
                let m = list_marker(child.style.list_style, counter);
                counter += 1;
                m
            } else {
                None
            };
            let opts = Opts {
                marker,
                ..Opts::default()
            };
            let r = self.layout_block(child, &bm, x, top, inh, opts);
            if first_baseline.is_none() {
                first_baseline = r.first_baseline;
            }
            cursor = top + r.height;
            prev_mb = Some(bm.margin[BOTTOM]);
        }
        (cursor + prev_mb.unwrap_or(0.0) - y, first_baseline)
    }

    // -------------------------------------------------------- posicionados

    /// Posiciona uma caixa absoluta/fixa no seu bloco contêiner (CSS 2.1 §10.3.7).
    fn layout_absolute(&mut self, p: Pending<'a>, cb: Rect) {
        let b = p.b;
        let s = &b.style;
        let cb = if s.position == Position::Fixed { self.viewport } else { cb };
        let (left, right) = (s.left.resolve(cb.w), s.right.resolve(cb.w));
        let (top, bottom) = (s.top.resolve(cb.h), s.bottom.resolve(cb.h));
        let border = [s.border(TOP), s.border(RIGHT), s.border(BOTTOM), s.border(LEFT)];
        let padding = s.padding.map(|l| l.resolve_or_zero(cb.w).max(0.0));
        let hx = padding[LEFT] + padding[RIGHT] + border[LEFT] + border[RIGHT];
        let vx = padding[TOP] + padding[BOTTOM] + border[TOP] + border[BOTTOM];
        let m = |l: Len| l.resolve(cb.w).unwrap_or(0.0);
        let (mt, mr, mb, ml) = (m(s.margin[TOP]), m(s.margin[RIGHT]), m(s.margin[BOTTOM]), m(s.margin[LEFT]));
        let content = |w: f32| if s.border_box { (w - hx).max(0.0) } else { w };

        // largura esticada entre left e right (só com width auto de verdade)
        let stretch_w = s.width.resolve(cb.w).is_none() && s.width_kw.is_none() && left.is_some() && right.is_some();
        let mut width = match (s.width.resolve(cb.w), left, right) {
            (Some(w), ..) => content(w),
            (None, Some(l), Some(r)) if stretch_w => (cb.w - l - r - ml - mr - hx).max(0.0),
            _ => {
                // encolhe para caber no conteúdo
                let avail = (cb.w - left.unwrap_or(0.0) - right.unwrap_or(0.0) - ml - mr - hx).max(0.0);
                let (mn, mx) = self.intrinsic(b);
                match s.width_kw {
                    Some(SizeKw::MaxContent) => (mx - hx).max(0.0),
                    Some(SizeKw::MinContent) => (mn - hx).max(0.0),
                    _ => (mx - hx).min(avail).max(mn - hx).max(0.0),
                }
            }
        };
        if let Some(maxw) = s.max_width.resolve(cb.w) {
            width = width.min(content(maxw));
        }
        if let Some(minw) = s.min_width.resolve(cb.w) {
            width = width.max(content(minw));
        }
        let bw = width + hx;
        // left e right fixos com largura que não estica: as margens auto
        // ficam com a sobra (as duas auto: centraliza)
        let (auto_l, auto_r) = (s.margin[LEFT] == Len::Auto, s.margin[RIGHT] == Len::Auto);
        let (ml, mr) = match (left, right) {
            (Some(l), Some(r)) if !stretch_w && (auto_l || auto_r) => {
                let free = cb.w - l - r - bw;
                match (auto_l, auto_r) {
                    (true, true) => ((free / 2.0).max(0.0), (free / 2.0).max(0.0)),
                    (true, false) => ((free - mr).max(0.0), mr),
                    _ => (ml, (free - ml).max(0.0)),
                }
            }
            _ => (ml, mr),
        };
        let x = match (left, right) {
            (Some(l), _) => cb.x + l,
            (None, Some(r)) => cb.x + cb.w - r - bw - ml - mr,
            _ => p.x,
        };
        let stretch_h = s.height.resolve(cb.h).is_none() && s.height_kw.is_none() && top.is_some() && bottom.is_some();
        let forced_h = match (s.height.resolve(cb.h), top, bottom) {
            (Some(h), ..) => Some(if s.border_box { (h - vx).max(0.0) } else { h }),
            (None, Some(t), Some(bo)) if stretch_h => Some((cb.h - t - bo - mt - mb - vx).max(0.0)),
            _ => None,
        };
        let y0 = match top {
            Some(t) => cb.y + t + mt,
            None => p.y + mt,
        };
        let bm = BoxModel {
            margin: [mt, mr, mb, ml],
            border,
            padding,
            content_w: width,
        };
        let order = p.order;
        let mark = self.mark();
        let opts = Opts {
            forced_h,
            bfc: true,
            ..Opts::default()
        };
        let is_fixed = s.position == Position::Fixed;
        if is_fixed {
            self.in_fixed += 1;
        }
        let saved_ctx = std::mem::replace(&mut self.ctx_stack, p.ctx.clone());
        let key = self.layer_key(s.z_index, order);
        self.abs_order = Some(order);
        let r = self.layout_block(b, &bm, x, y0, &p.inh, opts);
        self.abs_order = None;
        self.ctx_stack = saved_ctx;
        if is_fixed {
            self.in_fixed -= 1;
        }
        let mut top_final = y0;
        let (auto_t, auto_b) = (s.margin[TOP] == Len::Auto, s.margin[BOTTOM] == Len::Auto);
        if top.is_none()
            && let Some(bo) = bottom
        {
            let target = cb.y + cb.h - bo - mb - r.height;
            self.shift_between(&mark, None, 0.0, target - y0);
            top_final = target;
        } else if let (Some(t), Some(bo)) = (top, bottom)
            && !stretch_h
            && (auto_t || auto_b)
        {
            // top e bottom fixos com altura que não estica: margens auto
            // dividem a sobra (inset: 0; margin: auto centraliza)
            let free = cb.h - t - bo - r.height;
            let target = match (auto_t, auto_b) {
                (true, true) => cb.y + t + (free / 2.0).max(0.0),
                (true, false) => cb.y + t + (free - mb).max(0.0),
                _ => y0,
            };
            if target != y0 {
                self.shift_between(&mark, None, 0.0, target - y0);
                top_final = target;
            }
        }
        // absolutos também estendem a página (os fixos não: ficam na janela)
        if s.position != Position::Fixed {
            self.abs_bottom = self.abs_bottom.max(top_final + r.height + mb);
        }
        let items = self.items.split_off(mark.items);
        self.layers.push(Layer {
            key,
            items,
            fixed: is_fixed || self.in_fixed > 0,
        });
    }

    // ------------------------------------------------- tamanhos intrínsecos

    /// (min-content, max-content) da caixa de borda, sem margens.
    fn intrinsic(&mut self, b: &'a LayoutBox) -> (f32, f32) {
        let key = b as *const LayoutBox as usize;
        if let Some(&v) = self.intrinsic_cache.get(&key) {
            return v;
        }
        let s = &b.style;
        let anon = matches!(b.kind, BoxKind::Anonymous);
        let extra = if anon {
            0.0
        } else {
            px(s.padding[LEFT]) + px(s.padding[RIGHT]) + s.border(LEFT) + s.border(RIGHT)
        };
        let content = |w: f32| if s.border_box { w } else { w + extra };
        let margins = |c: &LayoutBox| {
            if matches!(c.kind, BoxKind::Anonymous) {
                0.0
            } else {
                px(c.style.margin[LEFT]) + px(c.style.margin[RIGHT])
            }
        };
        let mut r = match (s.width, anon) {
            (Len::Px(w), false) => (content(w), content(w)),
            _ => {
                let (mn, mx) = if s.display.is_flex() && !anon {
                    let gap = px(s.column_gap);
                    let kids: Vec<&'a LayoutBox> = b.children.iter().filter(|c| !c.is_out_of_flow()).collect();
                    let (mut sum_min, mut sum_max, mut max_min, mut max_max) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                    for c in &kids {
                        let (a, z) = self.intrinsic(c);
                        let m = margins(c);
                        sum_min += a + m;
                        sum_max += z + m;
                        max_min = max_min.max(a + m);
                        max_max = max_max.max(z + m);
                    }
                    let gaps = gap * kids.len().saturating_sub(1) as f32;
                    if s.flex_direction.is_column() {
                        (max_min, max_max)
                    } else if s.flex_wrap {
                        (max_min, sum_max + gaps)
                    } else {
                        (sum_min + gaps, sum_max + gaps)
                    }
                } else if s.display == Display::Table && !anon {
                    let g = table_grid(self.doc, b);
                    if g.rows.is_empty() {
                        (0.0, 0.0)
                    } else {
                        let sp = if s.border_collapse { 0.0 } else { s.border_spacing };
                        let (mins, maxs) = self.table_columns(&g, sp);
                        let spacing = sp * (g.ncols + 1) as f32;
                        (mins.iter().sum::<f32>() + spacing, maxs.iter().sum::<f32>() + spacing)
                    }
                } else if has_inflow_blocks(b) || (s.display.is_grid() && !anon) {
                    let mut acc = (0.0f32, 0.0f32);
                    for c in b.children.iter().filter(|c| !c.is_out_of_flow()) {
                        let (a, z) = self.intrinsic(c);
                        let m = margins(c);
                        acc = (acc.0.max(a + m), acc.1.max(z + m));
                    }
                    acc
                } else {
                    self.inline_intrinsic(b)
                };
                (mn + extra, mx + extra)
            }
        };
        if !anon {
            if let Len::Px(m) = s.max_width {
                let m = content(m);
                r = (r.0.min(m), r.1.min(m));
            }
            if let Len::Px(m) = s.min_width {
                let m = content(m);
                r = (r.0.max(m), r.1.max(m));
            }
        }
        self.intrinsic_cache.insert(key, r);
        r
    }

    /// Larguras intrínsecas de conteúdo em linha: a palavra mais longa e a
    /// linha inteira sem quebras.
    fn inline_intrinsic(&mut self, b: &'a LayoutBox) -> (f32, f32) {
        let mut items = Vec::new();
        self.flatten(b, &mut items, &Inherited::default(), 1.0e6);
        let (mut widest, mut line, mut min) = (0.0f32, 0.0f32, 0.0f32);
        // `run`: largura desde a última oportunidade de quebra
        let mut run = 0.0f32;
        let mut space_before = false;
        let mut space_breaks = true;
        let mut space_w = 4.0;
        for it in &items {
            match it {
                Item::Text { text, style, .. } => {
                    let font = self.fonts.for_style(style);
                    let size = style.font_size;
                    space_w = self.fonts.advance(font, ' ', size);
                    let nowrap = matches!(style.white_space, WhiteSpace::NoWrap | WhiteSpace::Pre);
                    let keep_newlines =
                        matches!(style.white_space, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::PreLine);
                    let text = transform(text, style.text_transform);
                    let segments: Vec<&str> = if keep_newlines { text.split('\n').collect() } else { vec![&text] };
                    for (si, seg) in segments.iter().enumerate() {
                        if si > 0 {
                            widest = widest.max(line);
                            line = 0.0;
                            run = 0.0;
                            space_before = false;
                        }
                        if seg.starts_with(is_collapsible) {
                            space_before = true;
                            space_breaks = !nowrap;
                        }
                        for (k, word) in seg.split(is_collapsible).filter(|w| !w.is_empty()).enumerate() {
                            let w = self.fonts.measure(font, size, word);
                            if k > 0 {
                                space_before = true;
                                space_breaks = !nowrap;
                            }
                            if space_before && line > 0.0 {
                                line += space_w;
                                run = if space_breaks { 0.0 } else { run + space_w };
                            }
                            line += w;
                            run += w;
                            space_before = false;
                            min = min.max(run);
                        }
                        if seg.ends_with(is_collapsible) && !seg.trim().is_empty() {
                            space_before = true;
                            space_breaks = !nowrap;
                        }
                    }
                }
                Item::Atomic { w, margin, pct, .. } => {
                    let m = margin[1] + margin[3];
                    let w = w + m;
                    if space_before && line > 0.0 {
                        line += space_w;
                        run = if space_breaks { 0.0 } else { run + space_w };
                    }
                    line += w;
                    // largura em % (campo com width:100%) encolhe até 0
                    run += if *pct { m } else { w };
                    min = min.max(run);
                    space_before = false;
                }
                Item::Float(f, _) => {
                    let (a, z) = self.intrinsic(f);
                    let m = px(f.style.margin[LEFT]) + px(f.style.margin[RIGHT]);
                    line += z + m;
                    min = min.max(a + m);
                }
                Item::InlineBlock(ib, _) => {
                    let (a, z) = self.intrinsic(ib);
                    let m = px(ib.style.margin[LEFT]) + px(ib.style.margin[RIGHT]);
                    if space_before && line > 0.0 {
                        line += space_w;
                        run = if space_breaks { 0.0 } else { run + space_w };
                    }
                    line += z + m;
                    run += a + m;
                    min = min.max(run);
                    space_before = false;
                }
                Item::InlineStart(st, _) => {
                    let e = px(st.margin[LEFT]) + st.border(LEFT) + px(st.padding[LEFT]);
                    line += e;
                    run += e;
                }
                Item::InlineEnd(st, _) => {
                    let e = px(st.margin[RIGHT]) + st.border(RIGHT) + px(st.padding[RIGHT]);
                    line += e;
                    run += e;
                    min = min.max(run);
                }
                Item::Break => {
                    widest = widest.max(line);
                    line = 0.0;
                    run = 0.0;
                    space_before = false;
                }
                Item::Anchor(_) | Item::OutOfFlow(..) => {}
            }
        }
        (min, widest.max(line))
    }

    // ------------------------------------------------------------- flexbox

    fn layout_flex(
        &mut self,
        b: &'a LayoutBox,
        x: f32,
        y: f32,
        w: f32,
        definite_h: Option<f32>,
        inh: &Inherited,
    ) -> (f32, Option<f32>) {
        let min_h = self.flex_min_h.take();
        let mut kids: Vec<&'a LayoutBox> = Vec::new();
        for c in &b.children {
            if c.is_out_of_flow() {
                self.add_pending(c, x, y, inh);
            } else {
                kids.push(c);
            }
        }
        kids.sort_by_key(|c| c.style.order);
        if kids.is_empty() {
            return (0.0, None);
        }
        if b.style.flex_direction.is_column() {
            self.flex_min_h = min_h;
            self.flex_column(b, &kids, x, y, w, definite_h, inh)
        } else {
            self.flex_min_h = min_h;
            self.flex_row(b, &kids, x, y, w, definite_h, inh)
        }
    }

    fn flex_item(&mut self, c: &'a LayoutBox, cb_w: f32) -> FlexItem<'a> {
        let cs = &c.style;
        let anon = matches!(c.kind, BoxKind::Anonymous);
        let (border, padding, margin) = if anon {
            ([0.0; 4], [0.0; 4], [0.0; 4])
        } else {
            let m = |l: Len| l.resolve(cb_w).unwrap_or(0.0);
            (
                [cs.border(TOP), cs.border(RIGHT), cs.border(BOTTOM), cs.border(LEFT)],
                cs.padding.map(|p| p.resolve_or_zero(cb_w).max(0.0)),
                [m(cs.margin[TOP]), m(cs.margin[RIGHT]), m(cs.margin[BOTTOM]), m(cs.margin[LEFT])],
            )
        };
        let extra = padding[LEFT] + padding[RIGHT] + border[LEFT] + border[RIGHT];
        let content = |v: f32| if cs.border_box { (v - extra).max(0.0) } else { v };
        let (imin, imax) = self.intrinsic(c);
        let base = match cs.flex_basis {
            FlexBasis::Len(l) => l.resolve(cb_w).map(content),
            FlexBasis::Auto if !anon => cs.width.resolve(cb_w).map(content),
            _ => None,
        }
        .unwrap_or((imax - extra).max(0.0));
        // tamanho mínimo automático: o min-content (ou a largura dada, se menor)
        let auto_min = if cs.overflow_clip {
            0.0
        } else {
            let mc = (imin - extra).max(0.0);
            match cs.width.resolve(cb_w) {
                Some(wv) if !anon => mc.min(content(wv)),
                _ => mc,
            }
        };
        let min = if anon { auto_min } else { cs.min_width.resolve(cb_w).map(content).unwrap_or(auto_min) };
        let max = if anon {
            f32::INFINITY
        } else {
            cs.max_width.resolve(cb_w).map(content).unwrap_or(f32::INFINITY)
        }
        .max(min);
        FlexItem {
            b: c,
            margin,
            auto_l: !anon && cs.margin[LEFT] == Len::Auto,
            auto_r: !anon && cs.margin[RIGHT] == Len::Auto,
            border,
            padding,
            extra,
            base,
            min,
            max,
            size: base.clamp(min, max),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn flex_row(
        &mut self,
        b: &'a LayoutBox,
        kids: &[&'a LayoutBox],
        x: f32,
        y: f32,
        w: f32,
        definite_h: Option<f32>,
        inh: &Inherited,
    ) -> (f32, Option<f32>) {
        let min_h = self.flex_min_h.take();
        let s = &b.style;
        let gap = s.column_gap.resolve_or_zero(w);
        let row_gap = s.row_gap.resolve_or_zero(w);
        let reverse = s.flex_direction == crate::style::FlexDirection::RowReverse;
        let mut items: Vec<FlexItem<'a>> = kids.iter().map(|c| self.flex_item(c, w)).collect();

        // quebra em linhas
        let mut lines: Vec<std::ops::Range<usize>> = Vec::new();
        let mut start = 0;
        let mut used = 0.0;
        for i in 0..items.len() {
            let o = items[i].outer();
            if s.flex_wrap && i > start && used + gap + o > w {
                lines.push(start..i);
                start = i;
                used = 0.0;
            }
            used += if i > start { gap + o } else { o };
        }
        lines.push(start..items.len());

        let mut cur_y = y;
        let mut first_baseline = None;
        let single_line = lines.len() == 1;
        for range in lines {
            let n = range.len();
            let line = &mut items[range.clone()];
            // tamanhos flexíveis: distribui o espaço livre (uma passada + limites)
            let gaps = gap * (n.saturating_sub(1)) as f32;
            let outer_base: f32 =
                line.iter().map(|it| it.base + it.extra + it.margin[LEFT] + it.margin[RIGHT]).sum::<f32>() + gaps;
            let free = w - outer_base;
            if free > 0.0 {
                let total: f32 = line.iter().map(|it| it.b.style.flex_grow).sum();
                for it in line.iter_mut() {
                    let g = it.b.style.flex_grow;
                    it.size = if total > 0.0 { it.base + free * g / total.max(1.0) } else { it.base };
                    it.size = it.size.clamp(it.min, it.max);
                }
            } else {
                let total: f32 = line.iter().map(|it| it.b.style.flex_shrink * it.base).sum();
                for it in line.iter_mut() {
                    let wgt = it.b.style.flex_shrink * it.base;
                    let cut = if total > 0.0 { -free * wgt / total } else { 0.0 };
                    it.size = (it.base - cut).clamp(it.min, it.max);
                }
            }
            let used: f32 = line.iter().map(FlexItem::outer).sum::<f32>() + gaps;
            let mut remaining = w - used;
            // margens auto engolem o espaço que sobra
            let autos: usize = line.iter().map(|it| it.auto_l as usize + it.auto_r as usize).sum();
            if remaining > 0.0 && autos > 0 {
                let each = remaining / autos as f32;
                for it in line.iter_mut() {
                    if it.auto_l {
                        it.margin[LEFT] += each;
                    }
                    if it.auto_r {
                        it.margin[RIGHT] += each;
                    }
                }
                remaining = 0.0;
            }
            let (mut pos, between) = if remaining > 0.0 {
                match s.justify_content {
                    Justify::Start => (0.0, 0.0),
                    Justify::End => (remaining, 0.0),
                    Justify::Center => (remaining / 2.0, 0.0),
                    Justify::SpaceBetween if n > 1 => (0.0, remaining / (n - 1) as f32),
                    Justify::SpaceBetween => (0.0, 0.0),
                    Justify::SpaceAround => (remaining / n as f32 / 2.0, remaining / n as f32),
                    Justify::SpaceEvenly => (remaining / (n + 1) as f32, remaining / (n + 1) as f32),
                }
            } else {
                (0.0, 0.0)
            };

            // layout de cada item na sua posição
            let mut placed: Vec<(Mark, BlockResult, f32)> = Vec::with_capacity(n);
            // linha única com altura definida: o item esticado já sai com a
            // altura da linha (um filho com height: 100% acompanha)
            let line_h = if single_line { definite_h } else { None };
            for it in line.iter() {
                let outer = it.outer();
                let rel = if reverse { w - pos - outer } else { pos };
                let bm = BoxModel {
                    margin: it.margin,
                    border: it.border,
                    padding: it.padding,
                    content_w: it.size,
                };
                let mut opts = Opts::bfc();
                if let Some(lh) = line_h {
                    let st = &it.b.style;
                    if st.align_self.unwrap_or(s.align_items) == Align::Stretch
                        && st.height == Len::Auto
                        && st.height_kw.is_none()
                        && st.aspect_ratio.is_none()
                        && st.margin[TOP] != Len::Auto
                        && st.margin[BOTTOM] != Len::Auto
                        && !matches!(it.b.kind, BoxKind::Anonymous)
                    {
                        let vx = it.border[TOP] + it.border[BOTTOM] + it.padding[TOP] + it.padding[BOTTOM];
                        opts.forced_h = Some((lh - it.margin[TOP] - it.margin[BOTTOM] - vx).max(0.0));
                    }
                }
                let m = self.mark();
                let r = self.layout_block(it.b, &bm, x + rel, cur_y + it.margin[TOP], inh, opts);
                let outer_h = r.height + it.margin[TOP] + it.margin[BOTTOM];
                placed.push((m, r, outer_h));
                pos += outer + gap + between;
            }
            let mut line_cross = placed.iter().map(|p| p.2).fold(0.0f32, f32::max);
            if single_line && let Some(h) = definite_h {
                line_cross = h;
            } else if single_line && let Some(m) = min_h {
                // sem altura definida: a linha única cresce até o min-height
                line_cross = line_cross.max(m);
            }
            // alinhamento no eixo cruzado
            for k in 0..n {
                let it = &line[k];
                let align = it.b.style.align_self.unwrap_or(s.align_items);
                let outer_h = placed[k].2;
                let dy = match align {
                    Align::Center => (line_cross - outer_h) / 2.0,
                    Align::End => line_cross - outer_h,
                    Align::Stretch => {
                        if it.b.style.height == Len::Auto && outer_h < line_cross {
                            let h = line_cross - it.margin[TOP] - it.margin[BOTTOM];
                            self.stretch(placed[k].1.deco, h);
                        }
                        0.0
                    }
                    Align::Start | Align::Baseline => 0.0,
                };
                if dy != 0.0 {
                    let (before, after) = placed.split_at(k + 1);
                    let next = after.first().map(|p| &p.0);
                    self.shift_between(&before[k].0, next, 0.0, dy);
                }
                if first_baseline.is_none() {
                    first_baseline = placed[k].1.first_baseline.map(|b| b + dy);
                }
            }
            cur_y += line_cross + row_gap;
        }
        ((cur_y - row_gap - y).max(0.0), first_baseline)
    }

    #[allow(clippy::too_many_arguments)]
    fn flex_column(
        &mut self,
        b: &'a LayoutBox,
        kids: &[&'a LayoutBox],
        x: f32,
        y: f32,
        w: f32,
        definite_h: Option<f32>,
        inh: &Inherited,
    ) -> (f32, Option<f32>) {
        let min_h = self.flex_min_h.take();
        let s = &b.style;
        let gap = s.row_gap.resolve_or_zero(w);
        let mut cur = y;
        let mut placed: Vec<(Mark, BlockResult, &'a LayoutBox)> = Vec::new();
        let mut first_baseline = None;
        for (k, &c) in kids.iter().enumerate() {
            let it = self.flex_item(c, w);
            let cs = &c.style;
            let align = cs.align_self.unwrap_or(s.align_items);
            let avail = (w - it.margin[LEFT] - it.margin[RIGHT] - it.extra).max(0.0);
            let anon = matches!(c.kind, BoxKind::Anonymous);
            let content_w = match cs.width.resolve(w) {
                Some(v) if !anon => {
                    if cs.border_box {
                        (v - it.extra).max(0.0)
                    } else {
                        v
                    }
                }
                _ if align == Align::Stretch => avail,
                _ => {
                    let (mn, mx) = self.intrinsic(c);
                    (mx - it.extra).min(avail).max(mn - it.extra).max(0.0)
                }
            };
            let content_w = content_w.clamp(it.min.min(content_w), it.max.max(0.0));
            let outer_w = content_w + it.extra + it.margin[LEFT] + it.margin[RIGHT];
            let free = (w - outer_w).max(0.0);
            let dx = match (it.auto_l, it.auto_r, align) {
                (true, true, _) => free / 2.0,
                (true, false, _) => free,
                (false, true, _) => 0.0,
                (_, _, Align::Center) => free / 2.0,
                (_, _, Align::End) => free,
                _ => 0.0,
            };
            let bm = BoxModel {
                margin: it.margin,
                border: it.border,
                padding: it.padding,
                content_w,
            };
            if k > 0 {
                cur += gap;
            }
            let m = self.mark();
            let mut r = self.layout_block(c, &bm, x + dx, cur + it.margin[TOP], inh, Opts::bfc());
            // flex-basis maior que o conteúdo estica o item; menor não encolhe
            // (o tamanho mínimo automático é o próprio conteúdo)
            if let FlexBasis::Len(Len::Px(v)) = cs.flex_basis {
                let vert = it.border[TOP] + it.border[BOTTOM] + it.padding[TOP] + it.padding[BOTTOM];
                let want = if cs.border_box { v } else { v + vert };
                if want > r.height {
                    self.stretch(r.deco, want);
                    r.height = want;
                }
            }
            cur += it.margin[TOP] + r.height + it.margin[BOTTOM];
            if first_baseline.is_none() {
                first_baseline = r.first_baseline;
            }
            placed.push((m, r, c));
        }
        let mut content_h = cur - y;
        // sem altura definida, o min-height é o tamanho principal para
        // crescer e distribuir (justify-content)
        if let Some(h) = definite_h.or(min_h)
            && h > content_h
        {
            let free = h - content_h;
            let total: f32 = placed.iter().map(|p| p.2.style.flex_grow).sum();
            if total > 0.0 {
                // cresce os itens e empurra os seguintes
                let mut acc = 0.0;
                for k in 0..placed.len() {
                    if acc != 0.0 {
                        let (before, after) = placed.split_at(k + 1);
                        let next = after.first().map(|p| &p.0);
                        self.shift_between(&before[k].0, next, 0.0, acc);
                    }
                    let g = placed[k].2.style.flex_grow;
                    if g > 0.0 {
                        let extra = free * g / total.max(1.0);
                        let new_h = placed[k].1.height + extra;
                        self.stretch(placed[k].1.deco, new_h);
                        acc += extra;
                    }
                }
            } else {
                let n = placed.len() as f32;
                let offset = match s.justify_content {
                    Justify::End => free,
                    Justify::Center => free / 2.0,
                    Justify::SpaceAround => free / n / 2.0,
                    Justify::SpaceEvenly => free / (n + 1.0),
                    _ => 0.0,
                };
                if let Some(first) = placed.first() {
                    self.shift_between(&first.0, None, 0.0, offset);
                }
            }
            content_h = h;
        }
        (content_h, first_baseline)
    }

    // ---------------------------------------------------------------- grid

    fn layout_grid(
        &mut self,
        b: &'a LayoutBox,
        x: f32,
        y: f32,
        w: f32,
        definite_h: Option<f32>,
        inh: &Inherited,
    ) -> (f32, Option<f32>) {
        let s = &b.style;
        let cgap = s.column_gap.resolve_or_zero(w);
        let rgap = s.row_gap.resolve_or_zero(w);
        let mut kids: Vec<&'a LayoutBox> = Vec::new();
        for c in &b.children {
            if c.is_out_of_flow() {
                self.add_pending(c, x, y, inh);
            } else {
                kids.push(c);
            }
        }
        kids.sort_by_key(|c| c.style.order);
        if kids.is_empty() {
            return (0.0, None);
        }

        // trilhas das colunas (com repeat(auto-fill, ...))
        let mut tracks: Vec<TrackSize> = s.grid_columns.tracks.clone();
        if let Some(rep) = &s.grid_columns.auto_fill {
            let unit: f32 = rep
                .iter()
                .map(|t| track_fixed(t.min, w).or(track_fixed(t.max, w)).unwrap_or(0.0).max(1.0))
                .sum::<f32>()
                + cgap * rep.len() as f32;
            let fixed: f32 = tracks.iter().filter_map(|t| track_fixed(t.max, w)).sum();
            let n = (((w - fixed + cgap) / unit).floor() as usize).clamp(1, 64);
            for _ in 0..n {
                tracks.extend(rep.iter().copied());
            }
        }
        let areas = s.grid_areas.clone();
        let area_cols = areas.as_ref().map_or(0, |a| a.iter().map(Vec::len).max().unwrap_or(0));
        let mut ncols = tracks.len().max(area_cols).max(1);
        let auto = TrackSize {
            min: Track::Auto,
            max: Track::Auto,
        };
        tracks.resize(ncols, auto);

        // posiciona os itens
        let mut occupied: Vec<Vec<bool>> = Vec::new();
        let mut items: Vec<GridItem<'a>> = Vec::new();
        let mut cursor = (0usize, 0usize);
        let resolve = |line: GridLine, end: GridLine, n: usize| -> (Option<usize>, usize) {
            let idx = |v: i32| -> usize {
                if v > 0 {
                    (v - 1) as usize
                } else {
                    (n as i32 + 1 + v).max(0) as usize
                }
            };
            match (line, end) {
                (GridLine::Line(a), GridLine::Line(b)) => {
                    let (a, b) = (idx(a), idx(b));
                    (Some(a.min(b)), (a.max(b) - a.min(b)).max(1))
                }
                (GridLine::Line(a), GridLine::Span(sp)) => (Some(idx(a)), sp as usize),
                (GridLine::Line(a), GridLine::Auto) => (Some(idx(a)), 1),
                (GridLine::Span(sp), GridLine::Line(b)) => (Some(idx(b).saturating_sub(sp as usize)), sp as usize),
                (GridLine::Span(sp), _) => (None, sp as usize),
                (GridLine::Auto, GridLine::Span(sp)) => (None, sp as usize),
                (GridLine::Auto, GridLine::Line(b)) => (Some(idx(b).saturating_sub(1)), 1),
                _ => (None, 1),
            }
        };
        // linhas com nome: as do template e as implícitas das áreas (x-start/x-end)
        let mut col_names: Vec<(usize, Rc<str>)> = s.grid_columns.names.clone();
        let mut row_names: Vec<(usize, Rc<str>)> = s.grid_rows.names.clone();
        if let Some(areas) = &areas {
            let mut seen: Vec<&str> = Vec::new();
            for row in areas.iter() {
                for cell in row {
                    if cell == "." || seen.contains(&cell.as_str()) {
                        continue;
                    }
                    seen.push(cell);
                    let (mut r0, mut c0, mut r1, mut c1) = (usize::MAX, usize::MAX, 0, 0);
                    for (r, rr) in areas.iter().enumerate() {
                        for (ci, x) in rr.iter().enumerate() {
                            if x == cell {
                                (r0, c0, r1, c1) = (r0.min(r), c0.min(ci), r1.max(r), c1.max(ci));
                            }
                        }
                    }
                    col_names.push((c0, Rc::from(format!("{cell}-start"))));
                    col_names.push((c1 + 1, Rc::from(format!("{cell}-end"))));
                    row_names.push((r0, Rc::from(format!("{cell}-start"))));
                    row_names.push((r1 + 1, Rc::from(format!("{cell}-end"))));
                }
            }
        }
        // Name → Line: no início procura `x-start`, no fim `x-end`, depois `x`
        let named = |l: &GridLine, end: bool, is_col: bool| -> GridLine {
            let GridLine::Name(n) = l else { return l.clone() };
            let names = if is_col { &col_names } else { &row_names };
            let suffixed = format!("{n}{}", if end { "-end" } else { "-start" });
            let find = |want: &str| names.iter().filter(|(_, x)| &**x == want).map(|(i, _)| *i).min();
            match find(&suffixed).or_else(|| find(n)) {
                Some(i) => GridLine::Line(i as i32 + 1),
                None => GridLine::Auto,
            }
        };
        // colunas implícitas: item posto além das colunas explícitas ganha
        // trilhas novas (grid-auto-columns; sem ele, auto)
        let mut need = ncols;
        for &c in &kids {
            let cs = &c.style;
            let (c0, c1) = (named(&cs.grid_col.0, false, true), named(&cs.grid_col.1, true, true));
            if let (Some(col), span) = resolve(c0, c1, ncols) {
                need = need.max(col + span);
            }
        }
        if need > ncols {
            for i in ncols..need.min(64) {
                let t = if s.grid_auto_cols.is_empty() {
                    auto
                } else {
                    s.grid_auto_cols[(i - ncols) % s.grid_auto_cols.len()]
                };
                tracks.push(t);
            }
            ncols = tracks.len();
        }
        let ncols = ncols;
        let mark_cells = |occupied: &mut Vec<Vec<bool>>, r: usize, c: usize, rs: usize, cs: usize| {
            while occupied.len() < r + rs {
                occupied.push(vec![false; ncols]);
            }
            for row in occupied.iter_mut().skip(r).take(rs) {
                for cell in row.iter_mut().skip(c).take(cs) {
                    *cell = true;
                }
            }
        };
        for &c in &kids {
            let cs = &c.style;
            // área nomeada
            if let (Some(name), Some(areas)) = (&cs.grid_area, &areas) {
                let mut found: Option<(usize, usize, usize, usize)> = None;
                for (r, row) in areas.iter().enumerate() {
                    for (col, cell) in row.iter().enumerate() {
                        if cell == name {
                            found = Some(match found {
                                None => (r, col, r, col),
                                Some((r0, c0, r1, c1)) => (r0.min(r), c0.min(col), r1.max(r), c1.max(col)),
                            });
                        }
                    }
                }
                if let Some((r0, c0, r1, c1)) = found {
                    mark_cells(&mut occupied, r0, c0, r1 - r0 + 1, c1 - c0 + 1);
                    items.push(GridItem {
                        b: c,
                        row: r0,
                        col: c0,
                        rspan: r1 - r0 + 1,
                        cspan: c1 - c0 + 1,
                    });
                    continue;
                }
            }
            let (c0, c1) = (named(&cs.grid_col.0, false, true), named(&cs.grid_col.1, true, true));
            let (r0, r1) = (named(&cs.grid_row.0, false, false), named(&cs.grid_row.1, true, false));
            let (col, cspan) = resolve(c0, c1, ncols);
            let (row, rspan) = resolve(r0, r1, usize::MAX / 4);
            let cspan = cspan.min(ncols);
            let (row, col) = match (row, col) {
                (Some(r), Some(c)) => (r, c.min(ncols - cspan)),
                (r, c) => {
                    // posicionamento automático, linha por linha
                    let mut pos = match r {
                        Some(r) => (r, 0),
                        None => cursor,
                    };
                    loop {
                        // coluna explícita além da grade: fica na última que cabe
                        let c0 = c.map_or(pos.1, |c| c.min(ncols.saturating_sub(cspan)));
                        if c0 + cspan > ncols {
                            pos = (pos.0 + 1, 0);
                            continue;
                        }
                        let free = (0..rspan).all(|dr| {
                            (0..cspan).all(|dc| {
                                occupied
                                    .get(pos.0 + dr)
                                    .is_none_or(|row| !row.get(c0 + dc).copied().unwrap_or(false))
                            })
                        });
                        if free {
                            break (pos.0, c0);
                        }
                        pos = if c.is_some() || pos.1 + 1 >= ncols { (pos.0 + 1, 0) } else { (pos.0, pos.1 + 1) };
                    }
                }
            };
            mark_cells(&mut occupied, row, col, rspan, cspan);
            if s.grid_area.is_none() {
                cursor = (row, col + cspan);
            }
            items.push(GridItem {
                b: c,
                row,
                col,
                rspan,
                cspan,
            });
        }

        // larguras das colunas, como no Chrome (CSS Grid §12): cada trilha
        // começa no mínimo (fixo, ou o min-content dos itens) com um teto
        // (fixo, ou o max-content); a sobra vai por igual para as trilhas
        // abaixo do teto, depois para as fr na proporção, e por fim estica
        // as auto
        let avail = (w - cgap * (ncols - 1) as f32).max(0.0);
        let mut base = vec![0.0f32; ncols];
        let mut limit = vec![f32::INFINITY; ncols];
        let mut fr = vec![0.0f32; ncols];
        for (i, t) in tracks.iter().enumerate() {
            if let Some(v) = track_fixed(t.min, w) {
                base[i] = v.max(0.0);
            }
            match t.max {
                Track::Fr(f) => fr[i] = f.max(0.0),
                m => {
                    if let Some(v) = track_fixed(m, w) {
                        limit[i] = v.max(0.0);
                    }
                }
            }
        }
        // o mínimo automático de um item: o min-content, ou o min-width dele
        // (min-width: 0 ou overflow escondido deixam a trilha encolher)
        let auto_min = |st: &ComputedStyle, mn: f32| {
            if st.min_width != Len::Auto || st.overflow_clip {
                st.min_width.resolve(w).unwrap_or(0.0).max(0.0)
            } else {
                mn
            }
        };
        for it in items.iter().filter(|it| it.cspan == 1) {
            let i = it.col;
            let t = tracks[i];
            let st = &it.b.style;
            let m = px(st.margin[LEFT]) + px(st.margin[RIGHT]);
            let (mn, mx) = self.intrinsic(it.b);
            let (mn, mx) = (mn + m, mx + m);
            match t.min {
                Track::Auto => base[i] = base[i].max(auto_min(st, mn)),
                Track::MinContent => base[i] = base[i].max(mn),
                Track::MaxContent => base[i] = base[i].max(mx),
                _ => {}
            }
            if fr[i] == 0.0 {
                let grow = match t.max {
                    Track::Auto | Track::MaxContent => Some(mx),
                    Track::MinContent => Some(mn),
                    _ => None,
                };
                if let Some(g) = grow {
                    limit[i] = if limit[i].is_infinite() { g } else { limit[i].max(g) };
                }
            }
        }
        // itens de várias colunas: o que faltar do min-content (e do
        // max-content, no teto) vai por igual para as trilhas intrínsecas
        for it in items.iter().filter(|it| it.cspan > 1) {
            let span: Vec<usize> = (it.col..(it.col + it.cspan).min(ncols)).collect();
            let flex: Vec<usize> =
                span.iter().copied().filter(|&i| track_fixed(tracks[i].min, w).is_none() && fr[i] == 0.0).collect();
            if flex.is_empty() {
                continue;
            }
            let st = &it.b.style;
            let m = px(st.margin[LEFT]) + px(st.margin[RIGHT]);
            let (mn, mx) = self.intrinsic(it.b);
            let gaps = cgap * (span.len() - 1) as f32;
            let have: f32 = span.iter().map(|&i| base[i]).sum::<f32>() + gaps;
            let want = auto_min(st, mn) + m;
            if want > have {
                let each = (want - have) / flex.len() as f32;
                for &i in &flex {
                    base[i] += each;
                }
            }
            let lim: f32 = span.iter().map(|&i| if limit[i].is_infinite() { base[i] } else { limit[i] }).sum::<f32>() + gaps;
            if mx + m > lim {
                // primeiro as trilhas ainda sem teto (sem itens só delas)
                let open: Vec<usize> = flex.iter().copied().filter(|&i| limit[i].is_infinite()).collect();
                let to = if open.is_empty() { &flex } else { &open };
                let each = (mx + m - lim) / to.len() as f32;
                for &i in to {
                    limit[i] = if limit[i].is_infinite() { base[i] + each } else { limit[i] + each };
                }
            }
        }
        for i in 0..ncols {
            // auto sem itens: o teto é o próprio mínimo; fr crescem depois
            if limit[i].is_infinite() || limit[i] < base[i] || fr[i] > 0.0 {
                limit[i] = base[i];
            }
        }
        let mut widths = base.clone();
        let mut free = avail - widths.iter().sum::<f32>();
        // sobra: por igual para as trilhas abaixo do teto
        for _ in 0..ncols + 1 {
            if free <= 0.01 {
                break;
            }
            let open: Vec<usize> = (0..ncols).filter(|&i| widths[i] < limit[i] - 0.01).collect();
            if open.is_empty() {
                break;
            }
            let each = free / open.len() as f32;
            for &i in &open {
                let g = each.min(limit[i] - widths[i]);
                widths[i] += g;
                free -= g;
            }
        }
        if fr.iter().any(|&f| f > 0.0) {
            // fr: o resto na proporção; uma fr cujo mínimo passa da parte dela
            // fica no mínimo e sai da conta
            let mut flexible: Vec<bool> = fr.iter().map(|&f| f > 0.0).collect();
            for _ in 0..ncols + 1 {
                let fixed_used: f32 = (0..ncols).filter(|&i| !flexible[i]).map(|i| widths[i]).sum();
                let tot: f32 = (0..ncols).filter(|&i| flexible[i]).map(|i| fr[i]).sum();
                if tot <= 0.0 {
                    break;
                }
                // soma das fr abaixo de 1: só essa fração da sobra é usada
                let unit = ((avail - fixed_used) / tot.max(1.0)).max(0.0);
                let mut changed = false;
                for i in 0..ncols {
                    if flexible[i] && base[i] > unit * fr[i] {
                        flexible[i] = false;
                        widths[i] = base[i];
                        changed = true;
                    }
                }
                if !changed {
                    for i in 0..ncols {
                        if flexible[i] {
                            widths[i] = unit * fr[i];
                        }
                    }
                    break;
                }
            }
        } else if free > 0.01 {
            // sem fr: as trilhas auto esticam por igual (justify-content: normal)
            let auto_cols: Vec<usize> = (0..ncols).filter(|&i| tracks[i].max == Track::Auto).collect();
            if !auto_cols.is_empty() {
                let each = free / auto_cols.len() as f32;
                for &i in &auto_cols {
                    widths[i] += each;
                }
            }
        }
        let mut col_x = vec![x; ncols + 1];
        for i in 0..ncols {
            col_x[i + 1] = col_x[i] + widths[i] + cgap;
        }

        items.sort_by_key(|it| (it.row, it.col));
        let explicit_rows = s.grid_rows.tracks.len();
        let row_track = |r: usize| -> TrackSize {
            if r < explicit_rows {
                s.grid_rows.tracks[r]
            } else if s.grid_auto_rows.is_empty() {
                auto
            } else {
                s.grid_auto_rows[(r - explicit_rows) % s.grid_auto_rows.len()]
            }
        };
        // linhas com altura conhecida antes do layout (px, % e fr com a
        // altura do contêiner definida): calcula as linhas primeiro e monta
        // cada item já com a altura da célula (esticado de verdade, e um
        // filho com height: 100% acompanha)
        let all_rows = items.iter().map(|it| it.row + it.rspan).max().unwrap_or(0).max(explicit_rows);
        let row_fixed = |t: Track| -> Option<f32> {
            match t {
                Track::Len(Len::Px(v)) => Some(v),
                Track::Len(l @ (Len::Percent(_) | Len::Calc(..))) => definite_h.and_then(|h| l.resolve(h)),
                _ => None,
            }
        };
        let known: Option<Vec<f32>> = (all_rows > 0).then(|| {
            let mut fixed = vec![0.0f32; all_rows];
            let mut frs = vec![0.0f32; all_rows];
            for r in 0..all_rows {
                let t = row_track(r);
                match t.max {
                    Track::Fr(f) if definite_h.is_some() => frs[r] = f.max(0.0),
                    m => fixed[r] = row_fixed(m)?,
                }
            }
            let tot: f32 = frs.iter().sum();
            let free = (definite_h.unwrap_or(0.0) - fixed.iter().sum::<f32>() - rgap * (all_rows - 1) as f32).max(0.0);
            Some((0..all_rows).map(|r| if frs[r] > 0.0 { free * frs[r] / tot } else { fixed[r] }).collect::<Vec<f32>>())
        }).flatten();
        if let Some(heights) = known {
            let mut row_y = vec![y; all_rows + 1];
            for r in 0..all_rows {
                row_y[r + 1] = row_y[r] + heights[r] + rgap;
            }
            let mut first_baseline = None;
            for it in &items {
                let st = &it.b.style;
                let cw = col_x[it.col + it.cspan] - col_x[it.col] - cgap;
                let bm = match it.b.kind {
                    BoxKind::Anonymous => BoxModel::anonymous(cw),
                    _ => self.box_model(st, cw),
                };
                let cell_h = (row_y[it.row + it.rspan] - row_y[it.row] - rgap).max(0.0);
                let align = st.align_self.unwrap_or(s.align_items);
                let mut opts = Opts::bfc();
                if align == Align::Stretch
                    && st.height == Len::Auto
                    && st.height_kw.is_none()
                    && st.aspect_ratio.is_none()
                    && st.margin[TOP] != Len::Auto
                    && st.margin[BOTTOM] != Len::Auto
                    && !matches!(it.b.kind, BoxKind::Anonymous)
                {
                    let vx = bm.border[TOP] + bm.border[BOTTOM] + bm.padding[TOP] + bm.padding[BOTTOM];
                    opts.forced_h = Some((cell_h - bm.margin[TOP] - bm.margin[BOTTOM] - vx).max(0.0));
                }
                let m = self.mark();
                let res = self.layout_block(it.b, &bm, col_x[it.col], row_y[it.row] + bm.margin[TOP], inh, opts);
                let outer = res.height + bm.margin[TOP] + bm.margin[BOTTOM];
                let dy = match align {
                    Align::Center => (cell_h - outer) / 2.0,
                    Align::End => cell_h - outer,
                    _ => 0.0,
                };
                if dy > 0.0 {
                    self.shift_between(&m, None, 0.0, dy);
                }
                if first_baseline.is_none() {
                    first_baseline = res.first_baseline.map(|b| b + dy.max(0.0));
                }
            }
            return ((row_y[all_rows] - y - rgap).max(0.0), first_baseline);
        }

        // linhas: altura pelo conteúdo, item a item
        let nrows = items.iter().map(|it| it.row + 1).max().unwrap_or(0);
        let mut row_y = y;
        let mut first_baseline = None;
        let mut k = 0;
        for r in 0..nrows {
            let mut placed: Vec<(Mark, BlockResult, usize)> = Vec::new();
            while k < items.len() && items[k].row == r {
                let it = &items[k];
                let cw = col_x[it.col + it.cspan] - col_x[it.col] - cgap;
                let bm = match it.b.kind {
                    BoxKind::Anonymous => BoxModel::anonymous(cw),
                    _ => self.box_model(&it.b.style, cw),
                };
                let m = self.mark();
                let res = self.layout_block(it.b, &bm, col_x[it.col], row_y + bm.margin[TOP], inh, Opts::bfc());
                placed.push((m, res, k));
                k += 1;
            }
            let fixed_row = s.grid_rows.tracks.get(r).and_then(|t| track_fixed(t.max, w));
            let row_h = placed
                .iter()
                .filter(|p| items[p.2].rspan == 1)
                .map(|p| {
                    let it = &items[p.2];
                    p.1.height + px(it.b.style.margin[TOP]) + px(it.b.style.margin[BOTTOM])
                })
                .fold(fixed_row.unwrap_or(0.0), f32::max);
            for p in &placed {
                let it = &items[p.2];
                let align = it.b.style.align_self.unwrap_or(s.align_items);
                let mt = px(it.b.style.margin[TOP]);
                let outer = p.1.height + mt + px(it.b.style.margin[BOTTOM]);
                match align {
                    Align::Stretch if it.b.style.height == Len::Auto && it.rspan == 1 => {
                        self.stretch(p.1.deco, row_h - (outer - p.1.height));
                    }
                    Align::Center | Align::End => {
                        let dy = if align == Align::Center { (row_h - outer) / 2.0 } else { row_h - outer };
                        let idx = placed.iter().position(|q| q.2 == p.2).unwrap();
                        let next = placed.get(idx + 1).map(|q| &q.0);
                        self.shift_between(&p.0, next, 0.0, dy.max(0.0));
                    }
                    _ => {}
                }
                if first_baseline.is_none() {
                    first_baseline = p.1.first_baseline;
                }
            }
            row_y += row_h + if row_h > 0.0 { rgap } else { 0.0 };
        }
        ((row_y - y - rgap).max(0.0), first_baseline)
    }

    // -------------------------------------------------------------- tabelas

    /// Larguras mínima e máxima de cada coluna (caixas de borda das células).
    fn table_columns(&mut self, g: &TableGrid<'a>, sp: f32) -> (Vec<f32>, Vec<f32>) {
        let n = g.ncols.max(1);
        let mut mins = vec![0.0f32; n];
        let mut maxs = vec![0.0f32; n];
        let mut spans = Vec::new();
        for (_, cells) in &g.rows {
            let mut c = 0;
            for &(cell, span) in cells {
                if c >= n {
                    break;
                }
                let (mn, mx) = self.intrinsic(cell);
                let span = span.min(n - c);
                if span == 1 {
                    mins[c] = mins[c].max(mn);
                    maxs[c] = maxs[c].max(mx);
                } else {
                    spans.push((c, span, mn, mx));
                }
                c += span;
            }
        }
        // células com colspan: distribui o que faltar entre as colunas
        for (c, span, mn, mx) in spans {
            let inner = sp * (span - 1) as f32;
            let have_min: f32 = mins[c..c + span].iter().sum::<f32>() + inner;
            if mn > have_min {
                let each = (mn - have_min) / span as f32;
                mins[c..c + span].iter_mut().for_each(|m| *m += each);
            }
            let have_max: f32 = maxs[c..c + span].iter().sum::<f32>() + inner;
            if mx > have_max {
                let each = (mx - have_max) / span as f32;
                maxs[c..c + span].iter_mut().for_each(|m| *m += each);
            }
        }
        for i in 0..n {
            maxs[i] = maxs[i].max(mins[i]);
        }
        (mins, maxs)
    }

    fn layout_table(&mut self, b: &'a LayoutBox, x: f32, y: f32, w: f32, inh: &Inherited) -> (f32, Option<f32>) {
        let g = table_grid(self.doc, b);
        if g.rows.is_empty() {
            return if has_inflow_blocks(b) {
                self.layout_block_children(b, x, y, w, inh)
            } else {
                self.layout_inline(b, x, y, w, inh)
            };
        }
        let s = &b.style;
        let sp = if s.border_collapse { 0.0 } else { s.border_spacing };
        let n = g.ncols.max(1);
        let (mins, maxs) = self.table_columns(&g, sp);
        let avail = (w - sp * (n + 1) as f32).max(0.0);
        let (smin, smax): (f32, f32) = (mins.iter().sum(), maxs.iter().sum());
        let widths: Vec<f32> = if smax <= avail {
            if smax > 0.0 {
                maxs.iter().map(|m| m + (avail - smax) * m / smax).collect()
            } else {
                vec![avail / n as f32; n]
            }
        } else if smin < avail {
            let k = (avail - smin) / (smax - smin).max(0.001);
            (0..n).map(|i| mins[i] + (maxs[i] - mins[i]) * k).collect()
        } else {
            mins.clone()
        };

        let mut cy = y;
        let mut first_baseline = None;
        for cap in &g.captions {
            let bm = self.box_model(&cap.style, w);
            let r = self.layout_block(cap, &bm, x, cy + bm.margin[TOP], inh, Opts::bfc());
            cy += bm.margin[TOP] + r.height + bm.margin[BOTTOM];
        }
        cy += sp;
        for (tr, cells) in &g.rows {
            // fundo da linha, atrás das células
            let row_bg = tr
                .filter(|t| t.style.visible && !t.style.background.is_transparent())
                .map(|t| {
                    self.items.push(DisplayItem::Rect {
                        rect: Rect::default(),
                        color: t.style.background,
                    });
                    (self.items.len() - 1, t.style.background)
                });
            let row_min = tr.and_then(|t| match t.style.height {
                Len::Px(v) => Some(v),
                _ => None,
            });
            let mut cx = x + sp;
            let mut col = 0;
            let mut placed: Vec<(Mark, BlockResult, VAlign)> = Vec::new();
            for &(cell, span) in cells {
                if col >= n {
                    break;
                }
                let span = span.min(n - col);
                let cw = widths[col..col + span].iter().sum::<f32>() + sp * (span - 1) as f32;
                col += span;
                let cs = &cell.style;
                let border = [cs.border(TOP), cs.border(RIGHT), cs.border(BOTTOM), cs.border(LEFT)];
                let padding = cs.padding.map(|p| p.resolve_or_zero(w).max(0.0));
                let extra = padding[LEFT] + padding[RIGHT] + border[LEFT] + border[RIGHT];
                let bm = BoxModel {
                    margin: [0.0; 4],
                    border,
                    padding,
                    content_w: (cw - extra).max(0.0),
                };
                let m = self.mark();
                let r = self.layout_block(cell, &bm, cx, cy, inh, Opts::bfc());
                placed.push((m, r, cs.vertical_align));
                cx += cw + sp;
            }
            let row_h = placed.iter().map(|p| p.1.height).fold(row_min.unwrap_or(0.0), f32::max);
            for k in 0..placed.len() {
                let (h, va) = (placed[k].1.height, placed[k].2);
                let dy = match va {
                    VAlign::Middle => (row_h - h) / 2.0,
                    VAlign::Bottom => row_h - h,
                    _ => 0.0,
                };
                if dy > 0.0 {
                    let (before, after) = placed.split_at(k + 1);
                    let next = after.first().map(|p| &p.0);
                    self.shift_between(&before[k].0, next, 0.0, dy);
                }
                self.stretch_to(placed[k].1.deco, Some(cy), row_h);
                if first_baseline.is_none() {
                    first_baseline = placed[k].1.first_baseline.map(|b| b + dy.max(0.0));
                }
            }
            if let Some((i, color)) = row_bg {
                self.items[i] = DisplayItem::Rect {
                    rect: Rect {
                        x: x + sp,
                        y: cy,
                        w: (w - 2.0 * sp).max(0.0),
                        h: row_h,
                    },
                    color,
                };
            }
            cy += row_h + sp;
        }
        (cy - y, first_baseline)
    }

    // ------------------------------------------------------------- em linha

    fn layout_inline(&mut self, b: &'a LayoutBox, x: f32, y: f32, w: f32, inh: &Inherited) -> (f32, Option<f32>) {
        let mut items = Vec::new();
        self.flatten(b, &mut items, inh, w);
        let floats = self.bfc_floats().to_vec();
        let mut lb = LineBuilder::new(self.fonts, &b.style, x, y, w, floats);
        let mut anchors: Vec<(String, usize)> = Vec::new();
        let mut out_of_flow: Vec<(&'a LayoutBox, Inherited, usize, f32)> = Vec::new();
        // inline-blocks já montados, esperando a posição final na linha
        let mut held: Vec<(Vec<DisplayItem>, Mark, Mark, f32, f32)> = Vec::new();
        for item in items {
            match item {
                Item::Text { text, style, inh } => lb.push_text(&text, &style, &inh),
                Item::Break => lb.break_line(true),
                Item::Atomic { w, h, margin, paint, label, style, inh, .. } => lb.push_box(w, h, margin, paint, label, &style, &inh),
                Item::InlineStart(style, id) => lb.open_inline(id, style),
                Item::InlineEnd(_, id) => lb.close_inline(id),
                Item::InlineBlock(ib, inh) => {
                    let bm = self.shrink_bm(ib, w, false);
                    let start = self.mark();
                    let r = self.layout_block(ib, &bm, x, y + bm.margin[TOP], &inh, Opts::bfc());
                    let end = self.mark();
                    let items = self.items.split_off(start.items);
                    let outer_w = bm.content_w + bm.extra_h() + bm.margin[LEFT] + bm.margin[RIGHT];
                    let outer_h = bm.margin[TOP] + r.height + bm.margin[BOTTOM];
                    let base = match r.first_baseline {
                        Some(bl) if !ib.style.overflow_clip => (bl - y).clamp(0.0, outer_h),
                        _ => outer_h,
                    };
                    held.push((items, start, end, x, y));
                    lb.push_inline_block(outer_w, outer_h, base, held.len() - 1, &ib.style, &inh);
                }
                Item::Anchor(name) => anchors.push((name, lb.lines.len())),
                Item::OutOfFlow(bx, inh) => out_of_flow.push((bx, inh, lb.lines.len(), lb.x)),
                Item::Float(bx, inh) => {
                    // o float vai para a linha atual (ou a próxima, se ela já tem conteúdo)
                    let top = y + lb.top + lb.gap + if lb.cur.is_empty() { 0.0 } else { lb.strut };
                    let f = self.place_float(bx, x, w, top, &inh);
                    lb.add_float(f);
                }
            }
        }
        let lines = lb.finish();

        let mut cy = y;
        let mut first = None;
        let mut line_tops = Vec::with_capacity(lines.len());
        for line in &lines {
            cy += line.gap;
            line_tops.push(cy);
            let free = (line.avail - line.width).max(0.0);
            let offset = line.inset
                + match b.style.text_align {
                    TextAlign::Left => 0.0,
                    TextAlign::Center | TextAlign::WebkitCenter => free / 2.0,
                    TextAlign::Right => free,
                };
            let baseline = cy + line.above;
            first.get_or_insert(baseline);
            let line_h = line.above + line.below;
            for seg in &line.segs {
                self.paint_inline_seg(seg, x + offset, baseline);
            }
            for f in &line.frags {
                let fx = x + offset + f.x;
                if let Some(href) = &f.inh.link {
                    let list = if self.in_fixed > 0 { &mut self.fixed_links } else { &mut self.links };
                    list.push((
                        Rect { x: fx, y: cy, w: f.w, h: line_h },
                        href.to_string(),
                    ));
                }
                if let Some(e) = f.inh.elem {
                    let r = Rect { x: fx, y: cy, w: f.w, h: line_h };
                    self.boxes.push((e, r));
                    if self.in_fixed > 0 {
                        self.fixed_nodes.insert(e);
                    }
                    if !f.inh.no_hit {
                        self.items.push(DisplayItem::Hit { rect: r, node: e });
                    }
                }
                if let FragKind::InlineBlock { h, base, hold } = &f.kind {
                    // move o inline-block já montado para o lugar dele
                    let top = match f.valign {
                        VAlign::Middle => baseline - f.mid - h / 2.0,
                        VAlign::Top => cy,
                        VAlign::Bottom => cy + line_h - h,
                        VAlign::Baseline => baseline - base,
                    };
                    let (items, start, end, tx, ty) = &mut held[*hold];
                    let (dx, dy) = (fx - *tx, top - *ty);
                    let mut moved = std::mem::take(items);
                    for it in &mut moved {
                        translate(it, dx, dy);
                    }
                    self.items.append(&mut moved);
                    let (mut a, mut z) = (start.clone(), end.clone());
                    a.items = 0;
                    z.items = 0;
                    self.shift_between(&a, Some(&z), dx, dy);
                    continue;
                }
                let b = match (&f.kind, f.valign) {
                    (FragKind::Box { base, .. }, VAlign::Top) => cy + base,
                    (FragKind::Box { h, base, .. }, VAlign::Bottom) => cy + line_h - h + base,
                    _ => baseline,
                };
                self.emit_frag(f, fx, b);
            }
            cy += line_h;
        }
        for (name, line) in anchors {
            self.anchors.push((name, line_tops.get(line).copied().unwrap_or(cy)));
        }
        for (bx, inh, line, lx) in out_of_flow {
            let top = line_tops.get(line).copied().unwrap_or(cy);
            self.add_pending(bx, x + lx, top, &inh);
        }
        (cy - y, first)
    }

    fn emit_frag(&mut self, f: &Frag, x: f32, baseline: f32) {
        let s = &f.style;
        if !s.visible {
            return;
        }
        match &f.kind {
            FragKind::Text(text) => {
                if s.color.is_transparent() {
                    return;
                }
                // text-shadow: uma cópia deslocada por baixo (sem desfoque)
                if let Some(sh) = s.text_shadow
                    && !sh.color.is_transparent()
                {
                    let mut c = sh.color;
                    if sh.blur > 0.0 {
                        c.a = (c.a as f32 * (1.0 / (1.0 + sh.blur / 4.0))).round() as u8;
                    }
                    self.items.push(DisplayItem::Text {
                        x: x + sh.x,
                        baseline: baseline + sh.y,
                        text: text.clone(),
                        font: f.font,
                        size: s.font_size,
                        color: c,
                    });
                }
                self.items.push(DisplayItem::Text {
                    x,
                    baseline,
                    text: text.clone(),
                    font: f.font,
                    size: s.font_size,
                    color: s.color,
                });
                let thick = (s.font_size / 15.0).max(1.0);
                if f.inh.underline {
                    self.items.push(DisplayItem::Rect {
                        rect: Rect { x, y: baseline + (s.font_size * 0.1).max(1.0), w: f.w, h: thick },
                        color: s.color,
                    });
                }
                if f.inh.line_through {
                    self.items.push(DisplayItem::Rect {
                        rect: Rect { x, y: baseline - s.font_size * 0.3, w: f.w, h: thick },
                        color: s.color,
                    });
                }
            }
            FragKind::InlineBlock { .. } => {}
            FragKind::Box { h, base, paint, label, .. } => {
                let top = if f.valign == VAlign::Middle { baseline - f.mid - h / 2.0 } else { baseline - base };
                let r = Rect { x, y: top, w: f.w, h: *h };
                if let AtomicPaint::Image(image) = paint {
                    // fundo atrás (PNG transparente) e cantos arredondados (avatares)
                    let radius = radius_of(s, r);
                    if s.visible && !s.background.is_transparent() {
                        self.items.push(if !radius.is_zero() {
                            DisplayItem::RoundRect { rect: r, radius, color: s.background }
                        } else {
                            DisplayItem::Rect { rect: r, color: s.background }
                        });
                    }
                    self.items.push(DisplayItem::Image {
                        rect: r,
                        image: image.clone(),
                        radius,
                    });
                    return;
                }
                if let AtomicPaint::Video(node) = paint {
                    self.items.push(DisplayItem::Video { rect: r, node: *node });
                    return;
                }
                if let AtomicPaint::Canvas(node) = paint {
                    self.items.push(DisplayItem::Canvas { rect: r, node: *node });
                    return;
                }
                let focused = f.inh.elem.is_some() && FOCUSED.with(|c| c.get()) == f.inh.elem;
                if matches!(paint, AtomicPaint::Control | AtomicPaint::TextArea | AtomicPaint::Button) {
                    if s.visible {
                        self.paint_control(s, r, paint, label, focused);
                    }
                    return;
                }
                let (fill, stroke) = match paint {
                    AtomicPaint::Placeholder => (Color::rgb(0xe8, 0xe8, 0xe8), Color::rgb(0xc8, 0xc8, 0xc8)),
                    AtomicPaint::Check(_) => (Color::WHITE, Color::rgb(0x76, 0x76, 0x76)),
                    _ => return,
                };
                let bw = 1.0;
                self.items.push(DisplayItem::Rect { rect: r, color: stroke });
                self.items.push(DisplayItem::Rect {
                    rect: Rect { x: r.x + bw, y: r.y + bw, w: (r.w - 2.0 * bw).max(0.0), h: (r.h - 2.0 * bw).max(0.0) },
                    color: fill,
                });
                if let AtomicPaint::Check(true) = paint {
                    self.items.push(DisplayItem::Rect {
                        rect: Rect { x: r.x + 3.0, y: r.y + 3.0, w: r.w - 6.0, h: r.h - 6.0 },
                        color: Color::rgb(0x00, 0x75, 0xff),
                    });
                }
            }
        }
    }

    /// `<input>`, `<select>`, `<textarea>`, botões de `<input>`: fundo, borda e
    /// cantos do CSS (a aparência padrão vem do ua.css); o texto fica na área de
    /// conteúdo, dentro do padding.
    fn paint_control(&mut self, s: &ComputedStyle, r: Rect, paint: &AtomicPaint, label: &Option<(String, Color)>, focused: bool) {
        let b = [s.border(TOP), s.border(RIGHT), s.border(BOTTOM), s.border(LEFT)];
        self.paint_decor(s, r, b);
        // anel de foco só em campos com borda (os sem borda costumam ter o
        // contorno desenhado pelo próprio site, num elemento em volta)
        if focused && matches!(paint, AtomicPaint::Control | AtomicPaint::TextArea) && b.iter().sum::<f32>() > 0.0 {
            let blue = Color::rgb(0x1a, 0x73, 0xe8);
            let radius = radius_of(s, r);
            if !radius.is_zero() {
                self.items.push(DisplayItem::RoundBorder { rect: r, radius, widths: [2.0; 4], color: blue });
            } else {
                for rect in [
                    Rect { x: r.x, y: r.y, w: r.w, h: 2.0 },
                    Rect { x: r.x, y: r.y + r.h - 2.0, w: r.w, h: 2.0 },
                    Rect { x: r.x, y: r.y, w: 2.0, h: r.h },
                    Rect { x: r.x + r.w - 2.0, y: r.y, w: 2.0, h: r.h },
                ] {
                    self.items.push(DisplayItem::Rect { rect, color: blue });
                }
            }
        }
        let (pt, pr, pb, pl) = (px(s.padding[TOP]), px(s.padding[RIGHT]), px(s.padding[BOTTOM]), px(s.padding[LEFT]));
        let inner = Rect {
            x: r.x + b[3],
            y: r.y + b[0],
            w: (r.w - b[1] - b[3]).max(0.0),
            h: (r.h - b[0] - b[2]).max(0.0),
        };
        let content = Rect {
            x: inner.x + pl,
            y: inner.y + pt,
            w: (inner.w - pl - pr).max(0.0),
            h: (inner.h - pt - pb).max(0.0),
        };
        let font = self.fonts.for_style(s);
        let m = self.fonts.line_metrics(font, s.font_size);
        // textarea: o texto começa no topo; nos outros, no meio
        let tall = matches!(paint, AtomicPaint::TextArea);
        let baseline = if tall {
            content.y + m.ascent
        } else {
            content.y + (content.h + m.ascent - m.descent) / 2.0
        };
        if let Some((text, color)) = label {
            let text_x = if matches!(paint, AtomicPaint::Button) {
                content.x + (content.w - self.fonts.measure(font, s.font_size, text)) / 2.0
            } else {
                content.x
            };
            self.items.push(DisplayItem::PushClip(inner));
            self.items.push(DisplayItem::Text {
                x: text_x,
                baseline,
                text: text.clone(),
                font,
                size: s.font_size,
                color: *color,
            });
            self.items.push(DisplayItem::PopClip);
        }
        // cursor de texto no fim do que foi digitado
        if focused && matches!(paint, AtomicPaint::Control | AtomicPaint::TextArea) {
            let typed = label.as_ref().filter(|(_, c)| *c == s.color).map(|(t, _)| t.as_str()).unwrap_or("");
            let cx = (content.x + self.fonts.measure(font, s.font_size, typed)).min(inner.x + inner.w - 2.0);
            let ch = (s.font_size * 1.2).min(inner.h - 2.0).max(1.0);
            let cy = if tall { baseline - m.ascent } else { inner.y + (inner.h - ch) / 2.0 };
            self.items.push(DisplayItem::Rect {
                rect: Rect { x: cx, y: cy, w: 1.5, h: ch },
                color: s.color,
            });
        }
    }

    /// Achata a subárvore em linha numa sequência de itens (texto, quebras,
    /// caixas atômicas, floats), levando junto decorações e links herdados.
    fn flatten(&self, b: &'a LayoutBox, out: &mut Vec<Item<'a>>, inh: &Inherited, avail: f32) {
        for c in &b.children {
            match c.kind {
                BoxKind::Text(n) => {
                    if let Some(t) = &c.text {
                        out.push(Item::Text {
                            text: t.to_string(),
                            style: c.style.clone(),
                            inh: inh.clone(),
                        });
                    } else if let NodeData::Text(t) = &self.doc.node(n).data {
                        out.push(Item::Text {
                            text: t.clone(),
                            style: c.style.clone(),
                            inh: inh.clone(),
                        });
                    }
                }
                BoxKind::Inline(n) => {
                    if self.doc.tag(n) == Some("br") && !c.generated {
                        out.push(Item::Break);
                        continue;
                    }
                    if !c.generated
                        && let Some(el) = self.doc.element(n)
                        && let Some(name) = el.id().or(if el.tag == "a" { el.attr("name") } else { None })
                    {
                        out.push(Item::Anchor(name.to_string()));
                    }
                    let mut inner = inh.enter(self.doc, Some(n), &c.style);
                    inner.elem = if c.generated { inh.elem } else { Some(n) };
                    let st = &c.style;
                    let decorated = st.visible
                        && (!st.background.is_transparent()
                            || matches!(st.bg_image, Some(BgImage::Linear(_)))
                            || st.border(LEFT) + st.border(RIGHT) + st.border(TOP) + st.border(BOTTOM) > 0.0
                            || px(st.padding[LEFT]) + px(st.padding[RIGHT]) > 0.0
                            || px(st.margin[LEFT]) != 0.0
                            || px(st.margin[RIGHT]) != 0.0);
                    if decorated {
                        let id = out.len();
                        out.push(Item::InlineStart(c.style.clone(), id));
                        self.flatten(c, out, &inner, avail);
                        out.push(Item::InlineEnd(c.style.clone(), id));
                    } else {
                        self.flatten(c, out, &inner, avail);
                    }
                }
                BoxKind::InlineBlock(_) => out.push(Item::InlineBlock(c, inh.clone())),
                BoxKind::Atomic(n) => self.atomic_item(n, c, out, inh, avail),
                BoxKind::Block(_) | BoxKind::Anonymous => {
                    if c.is_out_of_flow() {
                        out.push(Item::OutOfFlow(c, inh.clone()));
                    } else if c.is_float() {
                        out.push(Item::Float(c, inh.clone()));
                    }
                }
            }
        }
    }

    fn atomic_item(&self, n: NodeId, b: &LayoutBox, out: &mut Vec<Item<'a>>, inh: &Inherited, avail: f32) {
        let Some(el) = self.doc.element(n) else { return };
        let s = &b.style;
        let fs = s.font_size;
        let px = |l: Len| match l {
            Len::Px(v) => Some(v),
            _ => None,
        };
        let attr_px = |name: &str| {
            el.attr(name)
                .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
        };
        // medindo a largura natural (contêiner sem largura definida): % vale
        // como auto, senão um campo com width:100% mediria 100% de "infinito"
        let pct = matches!(s.width, Len::Percent(_) | Len::Calc(..));
        let w = if pct && avail >= 1.0e6 { None } else { s.width.resolve(avail) }.or_else(|| attr_px("width"));
        let h = px(s.height).or_else(|| attr_px("height")).map(|v| v.max(0.0));
        let font = self.fonts.for_style(s);
        let text_w = |t: &str| self.fonts.measure(font, fs, t);
        // controles de formulário: o CSS dá o tamanho do conteúdo (ou da borda,
        // com box-sizing: border-box); padding e borda vêm do estilo
        let extra_w = px(s.padding[LEFT]).unwrap_or(0.0) + px(s.padding[RIGHT]).unwrap_or(0.0) + s.border(LEFT) + s.border(RIGHT);
        let extra_h = px(s.padding[TOP]).unwrap_or(0.0) + px(s.padding[BOTTOM]).unwrap_or(0.0) + s.border(TOP) + s.border(BOTTOM);
        let line = s.line_height_px((fs * 1.2).round());
        let max_w = s.max_width.resolve(avail);
        let outer_w = |given: Option<f32>, content: f32| {
            let v = match given {
                Some(v) if s.border_box => v.max(extra_w),
                Some(v) => v + extra_w,
                None => content + extra_w,
            };
            match max_w {
                Some(m) if s.border_box => v.min(m.max(extra_w)),
                Some(m) => v.min(m + extra_w),
                None => v,
            }
        };
        let outer_h = |given: Option<f32>, content: f32| match given {
            Some(v) if s.border_box => v.max(extra_h),
            Some(v) => v + extra_h,
            None => content + extra_h,
        };
        let mut own = inh.clone();
        if !b.generated {
            own.elem = Some(n);
            own.no_hit = !s.pointer_events || !s.visible;
        }
        // margens (auto vale 0 em linha; % é da largura do contêiner)
        let m = |l: Len| l.resolve(avail).unwrap_or(0.0);
        let margin = [m(s.margin[TOP]), m(s.margin[RIGHT]), m(s.margin[BOTTOM]), m(s.margin[LEFT])];
        let push = |out: &mut Vec<Item<'a>>, w: f32, h: f32, paint: AtomicPaint, label: Option<(String, Color)>| {
            out.push(Item::Atomic {
                w,
                h,
                margin,
                pct,
                paint,
                label,
                style: s.clone(),
                inh: own.clone(),
            })
        };
        let gray = Color::rgb(0x75, 0x75, 0x75);
        match el.tag.as_str() {
            "img" if self.images.nodes.contains_key(&n) => {
                let img = &self.images.nodes[&n];
                let (iw, ih) = (img.width as f32, img.height as f32);
                let (mut w, mut h) = match (w, h) {
                    (Some(w), Some(h)) => (w, h),
                    (Some(w), None) => (w, w * ih / iw),
                    (None, Some(h)) => (h * iw / ih, h),
                    (None, None) => (iw, ih),
                };
                if let Some(max) = s.max_width.resolve(avail)
                    && w > max
                    && w > 0.0
                {
                    h *= max / w;
                    w = max;
                }
                push(out, w, h, AtomicPaint::Image(img.clone()), None);
            }
            "img" => match (w, h) {
                (Some(w), Some(h)) => push(out, w, h, AtomicPaint::Placeholder, None),
                _ => {
                    // sem tamanho conhecido: mostra o alt
                    let alt = el.attr("alt").unwrap_or("").trim();
                    if !alt.is_empty() {
                        out.push(Item::Text {
                            text: alt.to_string(),
                            style: s.clone(),
                            inh: inh.clone(),
                        });
                    } else if let Some(side) = w.or(h) {
                        push(out, side, side, AtomicPaint::Placeholder, None);
                    }
                }
            },
            "svg" => {
                // sem tamanho, como no Chrome: com viewBox, a largura do
                // contêiner e a proporção (medindo o contêiner, conta 0); sem
                // viewBox, 300×150; com um lado só, o outro vem da proporção
                let ratio = el.attr("viewbox").or_else(|| el.attr("viewBox")).and_then(|v| {
                    let n: Vec<f32> =
                        v.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|t| t.parse().ok()).collect();
                    (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0).then(|| n[2] / n[3])
                });
                let (w, h) = match (w, h, ratio) {
                    (Some(w), Some(h), _) => (w, h),
                    (Some(w), None, Some(r)) => (w, w / r),
                    (None, Some(h), Some(r)) => (h * r, h),
                    (Some(w), None, None) => (w, 150.0),
                    (None, Some(h), None) => (300.0, h),
                    (None, None, Some(r)) => {
                        let w = if avail < 1.0e6 { avail.max(0.0) } else { 0.0 };
                        (w, w / r)
                    }
                    (None, None, None) => (300.0, 150.0),
                };
                let paint = match inline_svg(self.doc, n, w, h, s.color) {
                    Some(img) => AtomicPaint::Image(img),
                    None => AtomicPaint::Invisible,
                };
                push(out, w, h, paint, None)
            }
            "input" => {
                let ty = el.attr("type").unwrap_or("text").to_ascii_lowercase();
                match ty.as_str() {
                    "hidden" => {}
                    "checkbox" | "radio" => {
                        push(out, 13.0, 13.0, AtomicPaint::Check(el.attr("checked").is_some()), None)
                    }
                    "submit" | "button" | "reset" => {
                        let label = el.attr("value").map(str::to_string).unwrap_or_else(|| {
                            match ty.as_str() {
                                "submit" => "Enviar",
                                "reset" => "Redefinir",
                                _ => "",
                            }
                            .to_string()
                        });
                        let (bw, bh) = (outer_w(w, text_w(&label)), outer_h(h, line));
                        push(out, bw, bh, AtomicPaint::Button, Some((label, s.color)));
                    }
                    "image" => push(out, w.unwrap_or(20.0), h.unwrap_or(20.0), AtomicPaint::Placeholder, None),
                    _ => {
                        let label = match el.attr("value").filter(|v| !v.is_empty()) {
                            Some(v) if ty == "password" => Some(("•".repeat(v.chars().count()), s.color)),
                            Some(v) => Some((v.to_string(), s.color)),
                            None => el.attr("placeholder").map(|p| (p.to_string(), s.placeholder_color.unwrap_or(gray))),
                        };
                        let size: f32 = el.attr("size").and_then(|v| v.parse().ok()).unwrap_or(20.0);
                        let (bw, bh) = (outer_w(w, size * fs * 0.55), outer_h(h, line));
                        push(out, bw, bh, AtomicPaint::Control, label);
                    }
                }
            }
            "select" => {
                let options = self.doc.descendants(n);
                let chosen = options
                    .iter()
                    .copied()
                    .filter(|&o| self.doc.tag(o) == Some("option"))
                    .find(|&o| self.doc.element(o).is_some_and(|e| e.attr("selected").is_some()))
                    .or_else(|| options.iter().copied().find(|&o| self.doc.tag(o) == Some("option")));
                let label = chosen
                    .map(|o| self.doc.text_content(o).split_whitespace().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let shown = format!("{label}  ▾");
                let (bw, bh) = (outer_w(w, text_w(&shown) + 4.0), outer_h(h, line));
                push(out, bw, bh, AtomicPaint::Control, Some((shown, s.color)));
            }
            "textarea" => {
                let cols: f32 = el.attr("cols").and_then(|v| v.parse().ok()).unwrap_or(20.0);
                let rows: f32 = el.attr("rows").and_then(|v| v.parse().ok()).unwrap_or(2.0);
                let text = self.doc.text_content(n);
                let label = if text.trim().is_empty() {
                    el.attr("placeholder").map(|p| (p.to_string(), s.placeholder_color.unwrap_or(gray)))
                } else {
                    Some((text.trim().to_string(), s.color))
                };
                let (bw, bh) = (outer_w(w, cols * fs * 0.55), outer_h(h, rows * line));
                push(out, bw, bh, AtomicPaint::TextArea, label);
            }
            "audio" => {
                if el.attr("controls").is_some() {
                    push(out, w.unwrap_or(300.0), h.unwrap_or(54.0), AtomicPaint::Placeholder, None);
                }
            }
            "video" => {
                // sem altura: segue a proporção do vídeo (ou 300×150 enquanto não se sabe)
                let natural = VIDEO_SIZES.with(|v| v.borrow().get(&n).copied()).filter(|s| s.0 > 0.0 && s.1 > 0.0);
                let (vw, vh) = match (w, h, natural) {
                    (Some(w), Some(h), _) => (w, h),
                    (Some(w), None, Some((nw, nh))) => (w, w * nh / nw),
                    (None, Some(h), Some((nw, nh))) => (h * nw / nh, h),
                    (None, None, Some((nw, nh))) => (nw.min(avail.max(1.0)), nw.min(avail.max(1.0)) * nh / nw),
                    (w, h, None) => (w.unwrap_or(300.0), h.unwrap_or(150.0)),
                };
                push(out, vw, vh, AtomicPaint::Video(n), None)
            }
            "canvas" | "iframe" => {
                // largura/altura do CSS; senão, as do atributo (padrão 300×150),
                // mantendo a proporção se só uma foi dada
                let (aw, ah) = (attr_px("width").unwrap_or(300.0), attr_px("height").unwrap_or(150.0));
                let (cw, ch) = match (s.width.resolve(avail), px(s.height)) {
                    (Some(cw), Some(ch)) => (cw, ch),
                    (Some(cw), None) => (cw, if aw > 0.0 { cw * ah / aw } else { ah }),
                    (None, Some(ch)) => (if ah > 0.0 { ch * aw / ah } else { aw }, ch),
                    (None, None) => (aw, ah),
                };
                push(out, cw, ch, AtomicPaint::Canvas(n), None)
            }
            _ => push(out, w.unwrap_or(300.0), h.unwrap_or(150.0), AtomicPaint::Placeholder, None),
        }
    }
}

// ------------------------------------------------------ quebra de linhas

enum Item<'a> {
    Text {
        text: String,
        style: Rc<ComputedStyle>,
        inh: Inherited,
    },
    Break,
    Atomic {
        w: f32,
        h: f32,
        /// Margens (topo, direita, baixo, esquerda), fora de `w` e `h`.
        margin: [f32; 4],
        /// Largura em %: na medida mínima (min-content) conta como 0.
        pct: bool,
        paint: AtomicPaint,
        label: Option<(String, Color)>,
        style: Rc<ComputedStyle>,
        inh: Inherited,
    },
    /// Marca a posição de um elemento com id dentro do texto.
    Anchor(String),
    /// Caixa absoluta/fixa no meio do texto (só a posição estática importa).
    OutOfFlow(&'a LayoutBox, Inherited),
    /// Float no meio do texto.
    Float(&'a LayoutBox, Inherited),
    /// inline-block no meio do texto.
    InlineBlock(&'a LayoutBox, Inherited),
    /// Começo e fim de um elemento em linha com fundo/borda/padding (id único).
    InlineStart(Rc<ComputedStyle>, usize),
    InlineEnd(Rc<ComputedStyle>, usize),
}

#[derive(Clone)]
enum AtomicPaint {
    Image(Arc<ImageData>),
    /// `<video>`: o quadro vem na hora de pintar.
    Video(NodeId),
    /// `<canvas>`: a imagem vem na hora de pintar (esticada na caixa).
    Canvas(NodeId),
    Invisible,
    Placeholder,
    Control,
    /// `<textarea>`: como `Control`, mas o texto começa no topo.
    TextArea,
    Button,
    Check(bool),
}

enum FragKind {
    Text(String),
    Box {
        h: f32,
        /// Do topo da caixa até a linha de base (texto dos campos; a borda de
        /// baixo nas imagens).
        base: f32,
        /// Margens de cima e de baixo (entram na altura da linha).
        mt: f32,
        mb: f32,
        paint: AtomicPaint,
        label: Option<(String, Color)>,
    },
    /// inline-block montado: altura, linha de base (do topo) e índice no `held`.
    InlineBlock {
        h: f32,
        base: f32,
        hold: usize,
    },
}

struct Frag {
    x: f32,
    w: f32,
    font: FontId,
    style: Rc<ComputedStyle>,
    inh: Inherited,
    kind: FragKind,
    valign: VAlign,
    /// Meia altura-x do contêiner (para vertical-align: middle).
    mid: f32,
}

/// Trecho de um elemento em linha decorado numa linha.
struct Seg {
    id: usize,
    style: Rc<ComputedStyle>,
    x0: f32,
    x1: f32,
    first: bool,
    last: bool,
}

struct OpenInline {
    id: usize,
    style: Rc<ComputedStyle>,
    start: f32,
    first: bool,
}

struct Line {
    frags: Vec<Frag>,
    width: f32,
    above: f32,
    below: f32,
    /// Recuo à esquerda (desviando de floats).
    inset: f32,
    /// Largura disponível para esta linha.
    avail: f32,
    /// Espaço antes da linha (pulando floats que não deixam espaço).
    gap: f32,
    segs: Vec<Seg>,
}

struct LineBuilder<'a> {
    fonts: &'a Fonts,
    container: &'a ComputedStyle,
    /// Origem e largura da caixa de conteúdo (coordenadas absolutas).
    ox: f32,
    oy: f32,
    width: f32,
    floats: Vec<FloatBox>,
    /// Altura de uma linha vazia (para testar floats).
    strut: f32,
    /// Topo da linha atual, relativo a `oy`, sem contar `gap`.
    top: f32,
    gap: f32,
    inset: f32,
    avail: f32,
    lines: Vec<Line>,
    cur: Vec<Frag>,
    x: f32,
    pending_space: f32,
    after_space: bool,
    /// O espaço antes da próxima palavra permite quebra (white-space dele).
    soft_break: bool,
    can_merge: bool,
    open: Vec<OpenInline>,
    line_segs: Vec<Seg>,
}

fn same_inherited(a: &Inherited, b: &Inherited) -> bool {
    a.underline == b.underline
        && a.line_through == b.line_through
        && match (&a.link, &b.link) {
            (None, None) => true,
            (Some(x), Some(y)) => Rc::ptr_eq(x, y) || x == y,
            _ => false,
        }
}

fn transform(text: &str, t: TextTransform) -> String {
    match t {
        TextTransform::None => text.to_string(),
        TextTransform::Upper => text.to_uppercase(),
        TextTransform::Lower => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for c in text.chars() {
                if start && c.is_alphabetic() {
                    out.extend(c.to_uppercase());
                    start = false;
                } else {
                    if c.is_whitespace() {
                        start = true;
                    }
                    out.push(c);
                }
            }
            out
        }
    }
}

/// Divide em pedaços alternados de espaço e não-espaço.
fn split_runs(s: &str, is_space: impl Fn(char) -> bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut cur_space: Option<bool> = None;
    for (i, c) in s.char_indices() {
        let sp = is_space(c);
        if cur_space.is_some_and(|v| v != sp) {
            out.push(&s[start..i]);
            start = i;
        }
        cur_space = Some(sp);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

impl<'a> LineBuilder<'a> {
    fn new(fonts: &'a Fonts, container: &'a ComputedStyle, ox: f32, oy: f32, width: f32, floats: Vec<FloatBox>) -> Self {
        let cfont = fonts.for_style(container);
        let m = fonts.line_metrics(cfont, container.font_size);
        let strut = container.line_height_px(m.normal).max(1.0);
        let mut lb = LineBuilder {
            fonts,
            container,
            ox,
            oy,
            width,
            floats,
            strut,
            top: 0.0,
            gap: 0.0,
            inset: 0.0,
            avail: width,
            lines: Vec::new(),
            cur: Vec::new(),
            x: 0.0,
            pending_space: 0.0,
            after_space: true,
            soft_break: false,
            can_merge: false,
            open: Vec::new(),
            line_segs: Vec::new(),
        };
        lb.place_line();
        lb
    }

    /// Acha a faixa livre (entre floats) para a linha que começa em `top`.
    fn place_line(&mut self) {
        self.gap = 0.0;
        for _ in 0..64 {
            let ly = self.oy + self.top + self.gap;
            let (mut l, mut r) = (0.0f32, self.width);
            let mut next: Option<f32> = None;
            for f in &self.floats {
                if f.rect.y < ly + self.strut && f.rect.y + f.rect.h > ly {
                    if f.left {
                        l = l.max(f.rect.x + f.rect.w - self.ox);
                    } else {
                        r = r.min(f.rect.x - self.ox);
                    }
                    let b = f.rect.y + f.rect.h;
                    next = Some(next.map_or(b, |n: f32| n.min(b)));
                }
            }
            let avail = r - l;
            match next {
                // não cabe quase nada ao lado do float: desce até ele acabar
                Some(n) if avail < self.width.min(60.0) && n > ly => {
                    self.gap = n - (self.oy + self.top);
                }
                _ => {
                    self.inset = l.max(0.0);
                    self.avail = avail.max(0.0);
                    return;
                }
            }
        }
    }

    fn open_inline(&mut self, id: usize, style: Rc<ComputedStyle>) {
        // o espaço antes do elemento fica do lado de fora da caixa
        if self.pending_space > 0.0 && !self.cur.is_empty() {
            self.x += self.pending_space;
        }
        self.pending_space = 0.0;
        self.x += px(style.margin[LEFT]);
        let start = self.x;
        self.x += style.border(LEFT) + px(style.padding[LEFT]);
        self.open.push(OpenInline {
            id,
            style,
            start,
            first: true,
        });
        self.can_merge = false;
    }

    fn close_inline(&mut self, id: usize) {
        let Some(pos) = self.open.iter().rposition(|o| o.id == id) else { return };
        let o = self.open.remove(pos);
        self.x += o.style.border(RIGHT) + px(o.style.padding[RIGHT]);
        self.line_segs.push(Seg {
            id,
            style: o.style.clone(),
            x0: o.start,
            x1: self.x,
            first: o.first,
            last: true,
        });
        self.x += px(o.style.margin[RIGHT]);
        self.can_merge = false;
    }

    fn push_inline_block(&mut self, w: f32, h: f32, base: f32, hold: usize, style: &Rc<ComputedStyle>, inh: &Inherited) {
        let mut sp = self.pending_space;
        let wrap = !matches!(self.container.white_space, WhiteSpace::Pre | WhiteSpace::NoWrap)
            || (self.soft_break && sp > 0.0);
        self.soft_break = false;
        if wrap && !self.cur.is_empty() && self.x + sp + w > self.avail {
            self.break_line(false);
            sp = 0.0;
        }
        self.x += sp;
        self.pending_space = 0.0;
        self.after_space = false;
        self.cur.push(Frag {
            x: self.x,
            w,
            font: 0,
            style: style.clone(),
            inh: inh.clone(),
            kind: FragKind::InlineBlock { h, base, hold },
            valign: style.vertical_align,
            mid: self.container.font_size * 0.3,
        });
        self.x += w;
        self.can_merge = false;
    }

    fn add_float(&mut self, f: FloatBox) {
        self.floats.push(f);
        if self.cur.is_empty() {
            self.place_line();
        }
    }

    fn push_text(&mut self, text: &str, style: &Rc<ComputedStyle>, inh: &Inherited) {
        let font = self.fonts.for_style(style);
        let size = style.font_size;
        let ws = style.white_space;
        let preserve = matches!(ws, WhiteSpace::Pre | WhiteSpace::PreWrap);
        let wrap = matches!(ws, WhiteSpace::Normal | WhiteSpace::PreWrap | WhiteSpace::PreLine);
        let keep_newlines = matches!(ws, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::PreLine);
        let text = transform(text, style.text_transform);
        let segments: Vec<&str> = if keep_newlines { text.split('\n').collect() } else { vec![&text] };

        for (i, seg) in segments.iter().enumerate() {
            if i > 0 {
                self.break_line(true);
            }
            if preserve {
                let seg = seg.trim_end_matches('\r').replace('\t', "    ");
                for tok in split_runs(&seg, |c| c == ' ') {
                    let w = self.fonts.measure(font, size, tok);
                    self.place_text(tok, w, style, font, inh, wrap && !tok.starts_with(' '), false);
                }
            } else {
                for tok in split_runs(seg, is_collapsible) {
                    if tok.chars().all(is_collapsible) {
                        if !self.cur.is_empty() && !self.after_space {
                            self.pending_space = self.fonts.advance(font, ' ', size);
                        }
                        self.after_space = true;
                        self.soft_break = wrap;
                    } else {
                        let w = self.fonts.measure(font, size, tok);
                        self.place_text(tok, w, style, font, inh, wrap, true);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place_text(
        &mut self,
        word: &str,
        w: f32,
        style: &Rc<ComputedStyle>,
        font: FontId,
        inh: &Inherited,
        wrap: bool,
        collapse: bool,
    ) {
        let mut sp = self.pending_space;
        let may_break = wrap || (self.soft_break && sp > 0.0);
        if may_break && !self.cur.is_empty() && self.x + sp + w > self.avail {
            self.break_line(false);
            sp = 0.0;
        }
        self.x += sp;
        self.pending_space = 0.0;
        self.after_space = false;
        self.soft_break = false;

        if self.can_merge
            && let Some(last) = self.cur.last_mut()
            && Rc::ptr_eq(&last.style, style)
            && same_inherited(&last.inh, inh)
            && let FragKind::Text(t) = &mut last.kind
        {
            if sp > 0.0 && collapse {
                t.push(' ');
            }
            t.push_str(word);
            last.w = self.x + w - last.x;
            self.x += w;
            return;
        }
        self.cur.push(Frag {
            x: self.x,
            w,
            font,
            style: style.clone(),
            inh: inh.clone(),
            kind: FragKind::Text(word.to_string()),
            valign: VAlign::Baseline,
            mid: 0.0,
        });
        self.x += w;
        self.can_merge = true;
    }

    #[allow(clippy::too_many_arguments)]
    fn push_box(
        &mut self,
        w: f32,
        h: f32,
        margin: [f32; 4],
        paint: AtomicPaint,
        label: Option<(String, Color)>,
        style: &Rc<ComputedStyle>,
        inh: &Inherited,
    ) {
        let mut sp = self.pending_space;
        let wrap = !matches!(style.white_space, WhiteSpace::Pre | WhiteSpace::NoWrap)
            || (self.soft_break && sp > 0.0);
        self.soft_break = false;
        let [mt, mr, mb, ml] = margin;
        if wrap && !self.cur.is_empty() && self.x + sp + ml + w + mr > self.avail {
            self.break_line(false);
            sp = 0.0;
        }
        self.x += sp + ml;
        self.pending_space = 0.0;
        self.after_space = false;
        // campos e botões alinham pela linha de base do texto de dentro (como
        // no Chrome); o resto, pela borda de baixo
        let base = match paint {
            AtomicPaint::Control | AtomicPaint::Button => {
                let m = self.fonts.line_metrics(self.fonts.for_style(style), style.font_size);
                let (bt, bb) = (style.border(TOP), style.border(BOTTOM));
                let (pt, pb) = (px(style.padding[TOP]), px(style.padding[BOTTOM]));
                let ch = (h - bt - bb - pt - pb).max(0.0);
                (bt + pt + (ch + m.ascent - m.descent) / 2.0).clamp(0.0, h.max(0.0))
            }
            _ => h,
        };
        self.cur.push(Frag {
            x: self.x,
            w,
            font: 0,
            style: style.clone(),
            inh: inh.clone(),
            kind: FragKind::Box { h, base, mt, mb, paint, label },
            valign: style.vertical_align,
            mid: self.container.font_size * 0.3,
        });
        self.x += w + mr;
        self.can_merge = false;
    }

    /// Fecha a linha atual. `forced` cria a linha mesmo vazia (<br>, \n em pre).
    fn break_line(&mut self, forced: bool) {
        if self.cur.is_empty() && self.line_segs.is_empty() && !forced {
            return;
        }
        let frags = std::mem::take(&mut self.cur);
        // "strut": toda linha tem pelo menos a altura da fonte do contêiner
        let c = self.container;
        let cfont = self.fonts.for_style(c);
        let m = self.fonts.line_metrics(cfont, c.font_size);
        let lh = c.line_height_px(m.normal);
        let half = (lh - (m.ascent + m.descent)) / 2.0;
        let (mut above, mut below) = (m.ascent + half, m.descent + half);
        // vertical-align: top/bottom: alinhados pela caixa da linha, que cresce para caber
        let (mut tall_top, mut tall_bottom) = (0.0f32, 0.0f32);
        for f in &frags {
            let full = match &f.kind {
                FragKind::Box { h, mt, mb, .. } => Some(h + mt + mb),
                FragKind::InlineBlock { h, .. } => Some(*h),
                FragKind::Text(_) => None,
            };
            if matches!(f.valign, VAlign::Top | VAlign::Bottom)
                && let Some(full) = full
            {
                if f.valign == VAlign::Top {
                    tall_top = tall_top.max(full);
                } else {
                    tall_bottom = tall_bottom.max(full);
                }
                continue;
            }
            match &f.kind {
                FragKind::Text(_) => {
                    let m = self.fonts.line_metrics(f.font, f.style.font_size);
                    let lh = f.style.line_height_px(m.normal);
                    let half = (lh - (m.ascent + m.descent)) / 2.0;
                    above = above.max(m.ascent + half);
                    below = below.max(m.descent + half);
                }
                FragKind::Box { h, .. } | FragKind::InlineBlock { h, .. } if f.valign == VAlign::Middle => {
                    above = above.max(h / 2.0 + f.mid);
                    below = below.max(h / 2.0 - f.mid);
                }
                FragKind::Box { h, base, mt, mb, .. } => {
                    above = above.max(base + mt);
                    below = below.max(h - base + mb);
                }
                FragKind::InlineBlock { h, base, .. } => {
                    above = above.max(*base);
                    below = below.max(h - base);
                }
            }
        }
        if tall_top > above + below {
            below = tall_top - above;
        }
        if tall_bottom > above + below {
            above = tall_bottom - below;
        }
        let mut segs = std::mem::take(&mut self.line_segs);
        let mut width = frags.last().map_or(0.0, |f| f.x + f.w);
        for sg in &segs {
            width = width.max(sg.x1);
        }
        // elementos decorados que continuam na próxima linha
        for o in &mut self.open {
            let lead = if o.first { o.style.border(LEFT) + px(o.style.padding[LEFT]) } else { 0.0 };
            if width > o.start + lead + 0.5 {
                segs.push(Seg {
                    id: o.id,
                    style: o.style.clone(),
                    x0: o.start,
                    x1: width,
                    first: o.first,
                    last: false,
                });
                o.first = false;
            }
            o.start = 0.0;
        }
        segs.sort_by_key(|sg| sg.id);
        let (above, below) = (above.max(0.0), below.max(0.0));
        self.lines.push(Line {
            frags,
            width,
            above,
            below,
            inset: self.inset,
            avail: self.avail,
            gap: self.gap,
            segs,
        });
        self.top += self.gap + above + below;
        self.x = 0.0;
        self.pending_space = 0.0;
        self.after_space = true;
        self.can_merge = false;
        self.place_line();
    }

    fn finish(mut self) -> Vec<Line> {
        self.break_line(false);
        self.lines
    }
}
