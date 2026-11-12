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

