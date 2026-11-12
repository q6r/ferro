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

