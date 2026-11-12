//! Canvas 2D: a imagem de um `<canvas>` e o rasterizador por trás do
//! `CanvasRenderingContext2D` (caminhos, traços, gradientes, imagens, texto,
//! recorte e os modos de composição).
//!
//! Os caminhos chegam já em coordenadas do dispositivo (o script aplica a
//! transformação ao montar o caminho). Os pixels ficam em RGBA 8 bits
//! pré-multiplicado; as contas de composição são feitas em f32.

use std::sync::Arc;

use crate::paint::ImageData;

/// Transformação afim `[a b c d e f]`: x' = a·x + c·y + e; y' = b·x + d·y + f.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat(pub [f32; 6]);

impl Mat {
    pub const IDENTITY: Mat = Mat([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    pub fn invert(&self) -> Option<Mat> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            return None;
        }
        let i = 1.0 / det;
        Some(Mat([d * i, -b * i, -c * i, a * i, (c * f - d * e) * i, (b * e - a * f) * i]))
    }

    /// Escala média (para larguras de traço e tamanho de texto).
    pub fn scale(&self) -> f32 {
        let [a, b, c, d, ..] = self.0;
        (a * d - b * c).abs().sqrt()
    }
}

/// Cor RGBA não pré-multiplicada, 0..1.
pub type Rgba = [f32; 4];

pub enum Style {
    Solid(Rgba),
    Linear { p0: (f32, f32), p1: (f32, f32), stops: Vec<(f32, Rgba)> },
    Radial { c0: (f32, f32), r0: f32, c1: (f32, f32), r1: f32, stops: Vec<(f32, Rgba)> },
    Conic { center: (f32, f32), angle: f32, stops: Vec<(f32, Rgba)> },
    Pattern { img: Arc<ImageData>, repeat_x: bool, repeat_y: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    SourceOver,
    DestinationOver,
    DestinationOut,
    DestinationIn,
    SourceIn,
    SourceOut,
    SourceAtop,
    DestinationAtop,
    Copy,
    Lighter,
    Xor,
    Multiply,
    Screen,
    Darken,
    Lighten,
    Overlay,
    Difference,
}

impl Op {
    pub fn parse(s: &str) -> Op {
        match s {
            "destination-over" => Op::DestinationOver,
            "destination-out" => Op::DestinationOut,
            "destination-in" => Op::DestinationIn,
            "source-in" => Op::SourceIn,
            "source-out" => Op::SourceOut,
            "source-atop" => Op::SourceAtop,
            "destination-atop" => Op::DestinationAtop,
            "copy" => Op::Copy,
            "lighter" | "plus-lighter" => Op::Lighter,
            "xor" => Op::Xor,
            "multiply" => Op::Multiply,
            "screen" => Op::Screen,
            "darken" => Op::Darken,
            "lighten" => Op::Lighten,
            "overlay" => Op::Overlay,
            "difference" | "exclusion" => Op::Difference,
            _ => Op::SourceOver,
        }
    }
}

/// Como pintar: estilo, transformação (para gradientes e padrões), alfa global
/// e modo de composição.
pub struct Paint {
    pub style: Style,
    /// Dispositivo → espaço do usuário (onde o gradiente foi definido).
    pub inv: Mat,
    pub alpha: f32,
    pub op: Op,
}

impl Paint {
