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
use crate::paint::{DisplayItem, ImageData};
use crate::style::{
    Align, BOTTOM, BgImage, BgSize, Clear, Color, ComputedStyle, ContentPart, Display, FlexBasis, Float, GridLine,
    Justify, LEFT, Len, ListStyle, Position, RIGHT, Styles, TOP, TextAlign, TextTransform, Track, TrackSize, VAlign,
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
                let atomic = LayoutBox::new(BoxKind::Atomic(node), style.clone(), Vec::new());
                if boxed {
                    // caixa posicionada/flutuante em volta do elemento substituído
                    return vec![LayoutBox::new(BoxKind::Block(node), style, vec![atomic])];
                }
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
}

/// Conteúdo posicionado ou flutuante, pintado por cima (ou por baixo, z < 0) do fluxo.
struct Layer {
    z: i32,
    order: usize,
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
        z_stack: vec![0],
        scrollers: Vec::new(),
        in_fixed: 0,
        fixed_nodes: Default::default(),
        fixed_links: Vec::new(),
        skip_bg,
        viewport,
        abs: vec![Vec::new()],
        layers: Vec::new(),
        next_layer: 0,
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
    let mut flow = split_layers(std::mem::take(&mut cx.items), 0, false, &mut extra);
    for l in layers.iter_mut() {
        l.items = split_layers(std::mem::take(&mut l.items), l.z, l.fixed, &mut extra);
    }
    layers.extend(extra);
    layers.sort_by_key(|l| (l.z, l.order));
    let mut items = Vec::new();
    let take = |l: &mut Layer, items: &mut Vec<DisplayItem>| {
        if l.fixed && !l.items.is_empty() {
            items.push(DisplayItem::FixedStart);
            items.append(&mut l.items);
            items.push(DisplayItem::FixedEnd);
        } else {
            items.append(&mut l.items);
        }
    };
    for l in layers.iter_mut().filter(|l| l.z < 0) {
        take(l, &mut items);
    }
    items.append(&mut flow);
    for l in layers.iter_mut().filter(|l| l.z >= 0) {
        take(l, &mut items);
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

