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
fn split_layers(items: Vec<DisplayItem>, base_z: i32, fixed: bool, out: &mut Vec<Layer>) -> Vec<DisplayItem> {
    if !items.iter().any(|it| matches!(it, DisplayItem::LayerStart { .. })) {
        return items
            .into_iter()
            .filter(|it| !matches!(it, DisplayItem::LayerEnd))
            .collect();
    }
    // pilha de trechos abertos: (z, ordem, itens); a base é o fluxo
    let mut stack: Vec<(i32, usize, Vec<DisplayItem>)> = vec![(base_z, 0, Vec::new())];
    for it in items {
        match it {
            DisplayItem::LayerStart { z, order } => {
                let parent_z = stack.last().map_or(base_z, |t| t.0);
                stack.push((z.unwrap_or(parent_z), order, Vec::new()));
            }
            DisplayItem::LayerEnd => {
                if stack.len() > 1 {
                    let (z, order, items) = stack.pop().unwrap();
                    out.push(Layer { z, order, items, fixed });
                }
            }
            other => stack.last_mut().unwrap().2.push(other),
        }
    }
    while stack.len() > 1 {
        let (z, order, items) = stack.pop().unwrap();
        out.push(Layer { z, order, items, fixed });
    }
    stack.pop().map(|t| t.2).unwrap_or_default()
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
    /// z-index dos contextos de empilhamento abertos.
    z_stack: Vec<i32>,
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
fn radius_of(s: &ComputedStyle, r: Rect) -> f32 {
    let m = r.w.min(r.h);
    match s.border_radius {
        Len::Px(v) => v,
        Len::Percent(p) => p / 100.0 * m,
        Len::Auto => 0.0,
    }
    .clamp(0.0, m / 2.0)
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

    fn add_pending(&mut self, b: &'a LayoutBox, x: f32, y: f32, inh: &Inherited) {
        let order = self.next_layer;
        self.next_layer += 1;
        let p = Pending {
            b,
            x,
            y,
            inh: inh.clone(),
            order,
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

    fn bg_item(&self, s: &ComputedStyle, r: Rect, radius: f32) -> Option<DisplayItem> {
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
                let pos = |l: Len, free: f32| match l {
                    Len::Percent(p) => free * p / 100.0,
                    Len::Px(v) => v,
                    Len::Auto => 0.0,
                };
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
        let radius = radius_of(s, r);
        if !s.background.is_transparent() && s.mask_image.is_none() {
            self.items.push(if radius > 0.0 {
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
            if radius > 0.0 {
                self.items.push(DisplayItem::RoundBorder {
                    rect: r,
                    radius,
                    width: bt.max(bb).max(bl).max(br),
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
        self.layers.push(Layer { z: 0, order, items, fixed: self.in_fixed > 0 });
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
        let w = (mx - extra).min(fill).max((mn - extra).max(0.0));
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
        if stacking {
            self.z_stack.push(s.z_index.unwrap_or(0));
        }
        if layered {
            let order = self.next_layer;
            self.next_layer += 1;
            self.items.push(DisplayItem::LayerStart { z: s.z_index, order });
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
                self.items.push(DisplayItem::Shadow { rect: Rect::default(), radius: 0.0, blur: 0.0, color: Color::TRANSPARENT });
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
        deco.round_border = has_border && s.border_radius != Len::Px(0.0);
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
                Len::Percent(p) => p / 100.0 * parent_h?,
                Len::Auto => return None,
            };
            Some(if s.border_box { (v - vert_extra).max(0.0) } else { v })
        };
        let definite_h = opts.forced_h.or_else(|| fixed(s.height));
        self.cb_heights.push(definite_h);

        let w = bm.content_w;
        let (mut content_h, first_baseline) = match s.display {
            _ if anon => {
                if has_inflow_blocks(b) {
                    self.layout_block_children(b, content_x, content_y, w, &inh)
                } else {
                    self.layout_inline(b, content_x, content_y, w, &inh)
                }
            }
            d if d.is_flex() => self.layout_flex(b, content_x, content_y, w, definite_h, &inh),
            d if d.is_grid() => self.layout_grid(b, content_x, content_y, w, &inh),
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
                        radius: (radius + sh.spread).max(0.0),
                        blur: sh.blur,
                        color: sh.color,
                    };
                }
            }
        }
        if let Some(i) = deco.bg {
            self.items[i] = if radius > 0.0 {
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
                width: bm.border.iter().copied().fold(0.0, f32::max),
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
            self.z_stack.pop();
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
            let bm = match child.kind {
                BoxKind::Anonymous => BoxModel::anonymous(w),
                _ if child.style.display == Display::Table => self.shrink_bm(child, w, true),
                _ => self.box_model(&child.style, w),
            };
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

        let mut width = match (s.width.resolve(cb.w), left, right) {
            (Some(w), ..) => content(w),
            (None, Some(l), Some(r)) => (cb.w - l - r - ml - mr - hx).max(0.0),
            _ => {
                // encolhe para caber no conteúdo
                let avail = (cb.w - left.unwrap_or(0.0) - right.unwrap_or(0.0) - ml - mr - hx).max(0.0);
                let (mn, mx) = self.intrinsic(b);
                (mx - hx).min(avail).max(mn - hx).max(0.0)
            }
        };
        if let Some(maxw) = s.max_width.resolve(cb.w) {
            width = width.min(content(maxw));
        }
        if let Some(minw) = s.min_width.resolve(cb.w) {
            width = width.max(content(minw));
        }
        let bw = width + hx;
        let x = match (left, right) {
            (Some(l), _) => cb.x + l,
            (None, Some(r)) => cb.x + cb.w - r - bw - ml - mr,
            _ => p.x,
        };
        let forced_h = match (s.height.resolve(cb.h), top, bottom) {
            (Some(h), ..) => Some(if s.border_box { (h - vx).max(0.0) } else { h }),
            (None, Some(t), Some(bo)) => Some((cb.h - t - bo - mt - mb - vx).max(0.0)),
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
        let r = self.layout_block(b, &bm, x, y0, &p.inh, opts);
        if is_fixed {
            self.in_fixed -= 1;
        }
        let mut top_final = y0;
        if top.is_none()
            && let Some(bo) = bottom
        {
            let target = cb.y + cb.h - bo - mb - r.height;
            self.shift_between(&mark, None, 0.0, target - y0);
            top_final = target;
        }
        // absolutos também estendem a página (os fixos não: ficam na janela)
        if s.position != Position::Fixed {
            self.abs_bottom = self.abs_bottom.max(top_final + r.height + mb);
        }
        let items = self.items.split_off(mark.items);
        self.layers.push(Layer {
            z: s.z_index.unwrap_or_else(|| self.z_stack.last().copied().unwrap_or(0)),
            order,
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
                Item::Atomic { w, .. } => {
                    if space_before && line > 0.0 {
                        line += space_w;
                        run = if space_breaks { 0.0 } else { run + space_w };
                    }
                    line += w;
                    run += w;
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
            self.flex_column(b, &kids, x, y, w, definite_h, inh)
        } else {
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
            for it in line.iter() {
                let outer = it.outer();
                let rel = if reverse { w - pos - outer } else { pos };
                let bm = BoxModel {
                    margin: it.margin,
                    border: it.border,
                    padding: it.padding,
                    content_w: it.size,
                };
                let m = self.mark();
                let r = self.layout_block(it.b, &bm, x + rel, cur_y + it.margin[TOP], inh, Opts::bfc());
                let outer_h = r.height + it.margin[TOP] + it.margin[BOTTOM];
                placed.push((m, r, outer_h));
                pos += outer + gap + between;
            }
            let mut line_cross = placed.iter().map(|p| p.2).fold(0.0f32, f32::max);
            if single_line && let Some(h) = definite_h {
                line_cross = h;
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
        if let Some(h) = definite_h
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

    fn layout_grid(&mut self, b: &'a LayoutBox, x: f32, y: f32, w: f32, inh: &Inherited) -> (f32, Option<f32>) {
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
        let ncols = tracks.len().max(area_cols).max(1);
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
            let (col, cspan) = resolve(cs.grid_col.0, cs.grid_col.1, ncols);
            let (row, rspan) = resolve(cs.grid_row.0, cs.grid_row.1, usize::MAX / 4);
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

        // larguras das colunas
        let avail = (w - cgap * (ncols - 1) as f32).max(0.0);
        // mínimo de cada trilha (minmax(45%, max-content) nunca fica abaixo de 45%)
        let min_w: Vec<f32> = tracks.iter().map(|t| track_fixed(t.min, w).unwrap_or(0.0).max(0.0)).collect();
        let mut widths = min_w.clone();
        let mut fr = vec![0.0f32; ncols];
        for (i, t) in tracks.iter().enumerate() {
            if let Some(v) = track_fixed(t.max, w) {
                widths[i] = v.max(track_fixed(t.min, w).unwrap_or(0.0));
            } else if let Track::Fr(f) = t.max {
                fr[i] = f;
                widths[i] = track_fixed(t.min, w).unwrap_or(0.0);
            }
        }
        // trilhas auto/min-content/max-content: pelo conteúdo dos itens
        for i in 0..ncols {
            let t = tracks[i];
            let content_track = matches!(t.max, Track::Auto | Track::MinContent | Track::MaxContent);
            let auto_min = matches!(t.min, Track::Auto | Track::MinContent) && fr[i] > 0.0;
            if !content_track && !auto_min {
                continue;
            }
            let mut need = 0.0f32;
            for it in items.iter().filter(|it| it.col == i && it.cspan == 1) {
                let (mn, mx) = self.intrinsic(it.b);
                let m = px(it.b.style.margin[LEFT]) + px(it.b.style.margin[RIGHT]);
                need = need.max(if t.max == Track::MinContent || auto_min { mn } else { mx } + m);
            }
            widths[i] = widths[i].max(need);
        }
        let used: f32 = widths.iter().sum();
        let fr_total: f32 = fr.iter().sum();
        if fr_total > 0.0 {
            let fixed_used: f32 = (0..ncols).filter(|&i| fr[i] == 0.0).map(|i| widths[i]).sum();
            let unit = ((avail - fixed_used) / fr_total).max(0.0);
            for i in 0..ncols {
                if fr[i] > 0.0 {
                    widths[i] = widths[i].max(unit * fr[i]);
                }
            }
        } else if used > avail {
            // trilhas auto encolhem até o min-content
            let auto_cols: Vec<usize> = (0..ncols).filter(|&i| track_fixed(tracks[i].max, w).is_none()).collect();
            let over = used - avail;
            // só encolhe o que está acima do mínimo da trilha
            let room: f32 = auto_cols.iter().map(|&i| (widths[i] - min_w[i]).max(0.0)).sum();
            if room > 0.0 {
                let cut = over.min(room);
                for &i in &auto_cols {
                    let r = (widths[i] - min_w[i]).max(0.0);
                    widths[i] -= cut * r / room;
                }
            }
        } else if used < avail {
            // sobra espaço: trilhas auto crescem por igual (justify-content: normal)
            let auto_cols: Vec<usize> = (0..ncols).filter(|&i| tracks[i].max == Track::Auto).collect();
            if !auto_cols.is_empty() {
                let each = (avail - used) / auto_cols.len() as f32;
                for &i in &auto_cols {
                    widths[i] += each;
                }
            }
        }
        let mut col_x = vec![x; ncols + 1];
        for i in 0..ncols {
            col_x[i + 1] = col_x[i] + widths[i] + cgap;
        }

        // linhas: altura pelo conteúdo, item a item
        items.sort_by_key(|it| (it.row, it.col));
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

