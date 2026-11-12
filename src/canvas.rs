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
    /// Cor do estilo no pixel (x, y) do dispositivo.
    fn color_at(&self, x: f32, y: f32) -> Rgba {
        let stops_at = |stops: &[(f32, Rgba)], t: f32| -> Rgba {
            if stops.is_empty() {
                return [0.0; 4];
            }
            if t <= stops[0].0 {
                return stops[0].1;
            }
            for w in stops.windows(2) {
                let ((t0, c0), (t1, c1)) = (w[0], w[1]);
                if t <= t1 {
                    let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 1.0 };
                    // interpolação pré-multiplicada (como os navegadores)
                    let a = c0[3] + (c1[3] - c0[3]) * f;
                    let mut out = [0.0, 0.0, 0.0, a];
                    for k in 0..3 {
                        let p = c0[k] * c0[3] + (c1[k] * c1[3] - c0[k] * c0[3]) * f;
                        out[k] = if a > 0.0 { p / a } else { 0.0 };
                    }
                    return out;
                }
            }
            stops[stops.len() - 1].1
        };
        match &self.style {
            Style::Solid(c) => *c,
            Style::Linear { p0, p1, stops } => {
                let (ux, uy) = self.inv.apply(x, y);
                let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 { ((ux - p0.0) * dx + (uy - p0.1) * dy) / len2 } else { 0.0 };
                stops_at(stops, t)
            }
            Style::Radial { c0, r0, c1, r1, stops } => {
                let (ux, uy) = self.inv.apply(x, y);
                // resolve |p - c(t)| = r(t), c(t) = c0 + t(c1-c0), r(t) = r0 + t(r1-r0); maior t com r(t) >= 0
                let (cdx, cdy, dr) = (c1.0 - c0.0, c1.1 - c0.1, r1 - r0);
                let (pdx, pdy) = (ux - c0.0, uy - c0.1);
                let a = cdx * cdx + cdy * cdy - dr * dr;
                let b = pdx * cdx + pdy * cdy + r0 * dr;
                let c = pdx * pdx + pdy * pdy - r0 * r0;
                let t = if a.abs() < 1e-6 {
                    if b.abs() < 1e-9 { return [0.0; 4] } else { c / (2.0 * b) }
                } else {
                    let disc = b * b - a * c;
                    if disc < 0.0 {
                        return [0.0; 4];
                    }
                    let s = disc.sqrt();
                    let (t1, t2) = ((b + s) / a, (b - s) / a);
                    let t = t1.max(t2);
                    if r0 + t * dr >= 0.0 { t } else { t1.min(t2) }
                };
                if r0 + t * dr < 0.0 {
                    return [0.0; 4];
                }
                stops_at(stops, t)
            }
            Style::Conic { center, angle, stops } => {
                let (ux, uy) = self.inv.apply(x, y);
                let a = (uy - center.1).atan2(ux - center.0) - angle;
                let t = (a / std::f32::consts::TAU).rem_euclid(1.0);
                stops_at(stops, t)
            }
            Style::Pattern { img, repeat_x, repeat_y } => {
                let (ux, uy) = self.inv.apply(x, y);
                let (w, h) = (img.width as f32, img.height as f32);
                if w <= 0.0 || h <= 0.0 {
                    return [0.0; 4];
                }
                let (mut sx, mut sy) = (ux.floor(), uy.floor());
                if *repeat_x {
                    sx = sx.rem_euclid(w);
                } else if sx < 0.0 || sx >= w {
                    return [0.0; 4];
                }
                if *repeat_y {
                    sy = sy.rem_euclid(h);
                } else if sy < 0.0 || sy >= h {
                    return [0.0; 4];
                }
                let i = (sy as usize * img.width as usize + sx as usize) * 4;
                let p = &img.rgba[i..i + 4];
                [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]
            }
        }
    }
}

