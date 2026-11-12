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

