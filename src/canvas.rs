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

/// Divide as polilinhas nos traços do tracejado.
fn dashed(polys: Vec<Poly>, dash: &[f32], offset: f32) -> Vec<Poly> {
    let total: f32 = dash.iter().sum();
    if dash.is_empty() || total <= 0.0 || dash.iter().any(|d| *d < 0.0 || !d.is_finite()) {
        return polys;
    }
    let mut out = Vec::new();
    for (mut pts, closed) in polys {
        if closed && pts.len() > 1 {
            pts.push(pts[0]);
        }
        // posição inicial no padrão
        let mut idx = 0;
        let mut left = dash[0];
        let mut on = true;
        let mut skip = offset.rem_euclid(total);
        while skip > 0.0 {
            if skip >= left {
                skip -= left;
                idx = (idx + 1) % dash.len();
                left = dash[idx];
                on = !on;
            } else {
                left -= skip;
                skip = 0.0;
            }
        }
        let mut cur: Vec<(f32, f32)> = if on { vec![pts[0]] } else { Vec::new() };
        for w in pts.windows(2) {
            let (mut a, b) = (w[0], w[1]);
            let mut seg = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
            while seg > 0.0 {
                if seg <= left {
                    left -= seg;
                    if on {
                        cur.push(b);
                    }
                    seg = 0.0;
                } else {
                    let t = left / seg;
                    let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                    if on {
                        cur.push(p);
                        out.push((std::mem::take(&mut cur), false));
                    } else {
                        cur = vec![p];
                    }
                    seg -= left;
                    a = p;
                    on = !on;
                    idx = (idx + 1) % dash.len();
                    left = dash[idx];
                }
            }
        }
        if on && cur.len() > 1 {
            out.push((cur, false));
        }
    }
    out
}

/// Polígonos que cobrem o traço (preenchidos juntos com "nonzero").
fn stroke_polys(polys: Vec<Poly>, st: &StrokeStyle) -> Vec<Poly> {
    let hw = st.width / 2.0;
    let polys = dashed(polys, &st.dash, st.dash_offset);
    let mut out = Vec::new();
    for (mut pts, closed) in polys {
        pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4);
        if closed && pts.len() > 2 && pts.first() == pts.last() {
            pts.pop();
        }
        if pts.len() == 1 {
            // caminho de um ponto só: com ponta redonda/quadrada vira um ponto
            match st.cap {
                Cap::Round => out.push(circle(pts[0], hw)),
                Cap::Square => {
                    let (x, y) = pts[0];
                    out.push(oriented(vec![(x - hw, y - hw), (x + hw, y - hw), (x + hw, y + hw), (x - hw, y + hw)]));
                }
                Cap::Butt => {}
            }
            continue;
        }
        let n = pts.len();
        let segs = if closed { n } else { n - 1 };
        let normal = |a: (f32, f32), b: (f32, f32)| {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
            ((-dy / l * hw, dx / l * hw), (dx / l, dy / l))
        };
        for i in 0..segs {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            let ((nx, ny), (ux, uy)) = normal(a, b);
            let (mut a, mut b) = (a, b);
            if !closed && st.cap == Cap::Square {
                if i == 0 {
                    a = (a.0 - ux * hw, a.1 - uy * hw);
                }
                if i == segs - 1 {
                    b = (b.0 + ux * hw, b.1 + uy * hw);
                }
            }
            out.push(oriented(vec![(a.0 + nx, a.1 + ny), (b.0 + nx, b.1 + ny), (b.0 - nx, b.1 - ny), (a.0 - nx, a.1 - ny)]));
        }
        // junções
        let joins: Vec<usize> = if closed { (0..n).collect() } else { (1..n - 1).collect() };
        for i in joins {
            let p = pts[i];
            let prev = pts[(i + n - 1) % n];
            let next = pts[(i + 1) % n];
            match st.join {
                Join::Round => out.push(circle(p, hw)),
                Join::Bevel | Join::Miter => {
                    let ((n1x, n1y), (u1x, u1y)) = normal(prev, p);
                    let ((n2x, n2y), (u2x, u2y)) = normal(p, next);
                    // lado de fora da curva
                    let cross = u1x * u2y - u1y * u2x;
                    let s = if cross > 0.0 { -1.0 } else { 1.0 };
                    let o1 = (p.0 + n1x * s, p.1 + n1y * s);
                    let o2 = (p.0 + n2x * s, p.1 + n2y * s);
                    let mut poly = vec![p, o1, o2];
                    if st.join == Join::Miter {
                        let cos = (u1x * u2x + u1y * u2y).clamp(-1.0, 1.0);
                        let half = ((1.0 + cos) / 2.0).sqrt(); // cos(θ/2) do ângulo entre as normais
                        if half > 1e-4 && 1.0 / half <= st.miter_limit {
                            let (mx, my) = (o1.0 + o2.0 - 2.0 * p.0, o1.1 + o2.1 - 2.0 * p.1);
                            let ml = (mx * mx + my * my).sqrt().max(1e-6);
                            let len = hw / half;
                            poly = vec![p, o1, (p.0 + mx / ml * len, p.1 + my / ml * len), o2];
                        }
                    }
                    out.push(oriented(poly));
                }
            }
        }
        if !closed && st.cap == Cap::Round {
            out.push(circle(pts[0], hw));
            out.push(circle(pts[n - 1], hw));
        }
    }
    out
}

/// A imagem de um canvas.
pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    /// RGBA pré-multiplicado.
    px: Vec<u8>,
    /// Recorte atual (cobertura 0..255 por pixel); None = sem recorte.
    clip: Option<Vec<u8>>,
    clip_stack: Vec<Option<Vec<u8>>>,
}

impl Bitmap {
    pub fn new(w: usize, h: usize) -> Bitmap {
        let (w, h) = (w.min(16384), h.min(16384));
        Bitmap { w, h, px: vec![0; w * h * 4], clip: None, clip_stack: Vec::new() }
    }

    /// Muda o tamanho (o conteúdo é apagado, como mandam os navegadores).
    pub fn resize(&mut self, w: usize, h: usize) {
        *self = Bitmap::new(w, h);
    }

    pub fn save(&mut self) {
        self.clip_stack.push(self.clip.clone());
    }

    pub fn restore(&mut self) {
        if let Some(c) = self.clip_stack.pop() {
            self.clip = c;
        }
    }

    /// Volta ao estado inicial do contexto (sem recorte).
    pub fn reset_state(&mut self) {
        self.clip = None;
        self.clip_stack.clear();
    }

    fn bounds(&self, polys: &[Poly], pad: f32) -> Option<(i32, i32, usize, usize)> {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (pts, _) in polys {
            for &(x, y) in pts {
                if x.is_finite() && y.is_finite() {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 > x1 {
            return None;
        }
        let bx = ((x0 - pad).floor() as i32).max(0);
        let by = ((y0 - pad).floor() as i32).max(0);
        let ex = ((x1 + pad).ceil() as i32).min(self.w as i32);
        let ey = ((y1 + pad).ceil() as i32).min(self.h as i32);
        if ex <= bx || ey <= by {
            return None;
        }
        Some((bx, by, (ex - bx) as usize, (ey - by) as usize))
    }

    /// Compõe um pixel: `src` não pré-multiplicada, `cov` = cobertura 0..1.
    fn blend(&mut self, x: usize, y: usize, src: Rgba, cov: f32, op: Op) {
        let i = (y * self.w + x) * 4;
        let mut cov = cov;
        if let Some(c) = &self.clip {
            cov *= c[y * self.w + x] as f32 / 255.0;
        }
        if cov <= 0.0 && !matches!(op, Op::Copy | Op::SourceIn | Op::DestinationIn | Op::SourceOut | Op::DestinationAtop) {
            return;
        }
        let sa = (src[3] * cov).clamp(0.0, 1.0);
        let s = [src[0] * sa, src[1] * sa, src[2] * sa, sa];
        let p = &mut self.px[i..i + 4];
        let d = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0];
        let da = d[3];
        let mut out = [0.0f32; 4];
        let pd = |fs: f32, fd: f32, out: &mut [f32; 4]| {
            for k in 0..4 {
                out[k] = s[k] * fs + d[k] * fd;
            }
        };
        match op {
            Op::SourceOver => pd(1.0, 1.0 - sa, &mut out),
            Op::DestinationOver => pd(1.0 - da, 1.0, &mut out),
            Op::DestinationOut => pd(0.0, 1.0 - sa, &mut out),
            Op::DestinationIn => pd(0.0, sa, &mut out),
            Op::SourceIn => pd(da, 0.0, &mut out),
            Op::SourceOut => pd(1.0 - da, 0.0, &mut out),
            Op::SourceAtop => pd(da, 1.0 - sa, &mut out),
            Op::DestinationAtop => pd(1.0 - da, sa, &mut out),
            Op::Copy => pd(1.0, 0.0, &mut out),
            Op::Lighter => {
                for k in 0..4 {
                    out[k] = (s[k] + d[k]).min(1.0);
                }
            }
            Op::Xor => pd(1.0 - da, 1.0 - sa, &mut out),
            Op::Multiply | Op::Screen | Op::Darken | Op::Lighten | Op::Overlay | Op::Difference => {
                // modo de mistura separável: Cs(1-ab) + Cb(1-as) + as·ab·B(cb, cs)
                for k in 0..3 {
                    let cs = src[k];
                    let cb = if da > 0.0 { d[k] / da } else { 0.0 };
                    let b = match op {
                        Op::Multiply => cs * cb,
                        Op::Screen => cs + cb - cs * cb,
                        Op::Darken => cs.min(cb),
                        Op::Lighten => cs.max(cb),
                        Op::Overlay => {
                            if cb <= 0.5 { 2.0 * cs * cb } else { 1.0 - 2.0 * (1.0 - cs) * (1.0 - cb) }
                        }
                        _ => (cs - cb).abs(),
                    };
                    out[k] = s[k] * (1.0 - da) + d[k] * (1.0 - sa) + sa * da * b;
                }
                out[3] = sa + da - sa * da;
            }
        }
        for k in 0..4 {
            p[k] = (out[k].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }

