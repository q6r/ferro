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

/// Um pedaço de caminho (coordenadas do dispositivo).
#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    Move(f32, f32),
    Line(f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Lê o caminho no formato compacto que o prelúdio manda: 0 x y (move),
/// 1 x y (linha), 2 c1x c1y c2x c2y x y (cúbica), 3 (fecha).
pub fn decode_path(nums: &[f64]) -> Vec<Cmd> {
    let mut out = Vec::new();
    let mut i = 0;
    let n = |j: usize| nums.get(j).copied().unwrap_or(0.0) as f32;
    while i < nums.len() {
        match nums[i] as i32 {
            0 => {
                out.push(Cmd::Move(n(i + 1), n(i + 2)));
                i += 3;
            }
            1 => {
                out.push(Cmd::Line(n(i + 1), n(i + 2)));
                i += 3;
            }
            2 => {
                out.push(Cmd::Cubic(n(i + 1), n(i + 2), n(i + 3), n(i + 4), n(i + 5), n(i + 6)));
                i += 7;
            }
            _ => {
                out.push(Cmd::Close);
                i += 1;
            }
        }
    }
    out
}

type Poly = (Vec<(f32, f32)>, bool);

/// Achata as curvas em polilinhas. Devolve (pontos, fechado).
fn flatten(path: &[Cmd]) -> Vec<Poly> {
    let mut out: Vec<Poly> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut start = (0.0, 0.0);
    let mut last = (0.0, 0.0);
    for c in path {
        match *c {
            Cmd::Move(x, y) => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), false));
                }
                last = (x, y);
                start = last;
                cur.push(last);
            }
            Cmd::Line(x, y) => {
                if cur.is_empty() {
                    cur.push(last);
                }
                last = (x, y);
                cur.push(last);
            }
            Cmd::Cubic(c1x, c1y, c2x, c2y, x, y) => {
                if cur.is_empty() {
                    cur.push(last);
                }
                let (p0, p1, p2, p3) = (last, (c1x, c1y), (c2x, c2y), (x, y));
                let d = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
                let len = d(p0, p1) + d(p1, p2) + d(p2, p3);
                let n = (len / 2.0).ceil().clamp(1.0, 128.0) as usize;
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let mt = 1.0 - t;
                    let (a, b, c, e) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
                    cur.push((
                        a * p0.0 + b * p1.0 + c * p2.0 + e * p3.0,
                        a * p0.1 + b * p1.1 + c * p2.1 + e * p3.1,
                    ));
                }
                last = (x, y);
            }
            Cmd::Close => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), true));
                }
                last = start;
                cur.push(last);
            }
        }
    }
    if cur.len() > 1 {
        out.push((cur, false));
    }
    out
}

/// Cobertura (0..1) de um conjunto de polígonos num retângulo `bx..bx+bw`,
/// `by..by+bh`: 4 sub-linhas por pixel, cobertura horizontal exata.
fn coverage(polys: &[Poly], evenodd: bool, bx: i32, by: i32, bw: usize, bh: usize) -> Vec<f32> {
    const SUB: usize = 4;
    let mut cov = vec![0.0f32; bw * bh];
    // arestas (y de cima, y de baixo, x em cima, dx/dy, sentido)
    let mut edges: Vec<(f32, f32, f32, f32, i32)> = Vec::new();
    for (pts, _) in polys {
        if pts.len() < 2 {
            continue;
        }
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            if a.1 == b.1 || !a.0.is_finite() || !a.1.is_finite() || !b.0.is_finite() || !b.1.is_finite() {
                continue;
            }
            let (top, bot, dir) = if a.1 < b.1 { (a, b, 1) } else { (b, a, -1) };
            let slope = (bot.0 - top.0) / (bot.1 - top.1);
            edges.push((top.1 - by as f32, bot.1 - by as f32, top.0 - bx as f32, slope, dir));
        }
    }
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut next = 0;
    let mut active: Vec<usize> = Vec::new();
    let mut xs: Vec<(f32, i32)> = Vec::new();
    let wf = bw as f32;
    for py in 0..bh {
        for sub in 0..SUB {
            let ys = py as f32 + (sub as f32 + 0.5) / SUB as f32;
            while next < edges.len() && edges[next].0 <= ys {
                active.push(next);
                next += 1;
            }
            active.retain(|&e| edges[e].1 > ys);
            if active.is_empty() {
                continue;
            }
            xs.clear();
            for &e in &active {
                let (y0, _, x0, slope, dir) = edges[e];
                if ys >= y0 {
                    xs.push((x0 + (ys - y0) * slope, dir));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            let row = py * bw;
            for k in 0..xs.len() {
                wind += xs[k].1;
                let inside = if evenodd { wind % 2 != 0 } else { wind != 0 };
                if inside && k + 1 < xs.len() {
                    let (xa, xb) = (xs[k].0.max(0.0), xs[k + 1].0.min(wf));
                    if xb <= xa {
                        continue;
                    }
                    let (ia, ib) = (xa.floor() as usize, (xb.ceil() as usize).min(bw));
                    for ix in ia..ib {
                        let overlap = xb.min(ix as f32 + 1.0) - xa.max(ix as f32);
                        if overlap > 0.0 {
                            cov[row + ix] += overlap / SUB as f32;
                        }
                    }
                }
            }
        }
    }
    cov
}

fn signed_area(p: &[(f32, f32)]) -> f32 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let (x0, y0) = p[i];
        let (x1, y1) = p[(i + 1) % p.len()];
        a += x0 * y1 - x1 * y0;
    }
    a / 2.0
}

/// Polígono sempre no mesmo sentido (para a união por "nonzero" funcionar).
fn oriented(mut p: Vec<(f32, f32)>) -> Poly {
    if signed_area(&p) < 0.0 {
        p.reverse();
    }
    (p, true)
}

fn circle(c: (f32, f32), r: f32) -> Poly {
    let n = ((r * std::f32::consts::TAU / 2.0).ceil() as usize).clamp(8, 96);
    oriented(
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                (c.0 + r * a.cos(), c.1 + r * a.sin())
            })
            .collect(),
    )
}

#[derive(Clone, Copy, PartialEq)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Join {
    Miter,
    Round,
    Bevel,
}

pub struct StrokeStyle {
    pub width: f32,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f32,
    pub dash: Vec<f32>,
    pub dash_offset: f32,
}

