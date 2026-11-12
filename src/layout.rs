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

