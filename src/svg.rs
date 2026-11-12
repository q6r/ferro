//! Renderizador de SVG próprio. Cobre o subconjunto usado por ícones e
//! logotipos: <path> (todos os comandos, inclusive arcos), <rect>, <circle>,
//! <ellipse>, <line>, <polyline>, <polygon>, <g>, `transform`, herança de
//! fill/stroke, `fill-rule` e antialias. Ainda não: gradientes, clipPath,
//! máscaras, texto e filtros.
//!
//! Preenchimento: varredura por linhas com regra nonzero/evenodd.
//! Traço: distância de cada pixel aos segmentos (pontas e junções redondas).

use crate::css;
use crate::html::{self, Token};
use crate::paint::ImageData;
use crate::style::{self, Color};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Paint {
    None,
    Color(Color),
    /// currentColor (e url(#...) enquanto gradientes não existem)
    Current,
}

#[derive(Clone, Copy, Debug)]
struct Matrix([f32; 6]); // a b c d e f

impl Matrix {
    const ID: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    fn mul(self, o: Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = o.0;
        Matrix([
            a * a2 + c * b2,
            b * a2 + d * b2,
            a * c2 + c * d2,
            b * c2 + d * d2,
            a * e2 + c * f2 + e,
            b * e2 + d * f2 + f,
        ])
    }

    fn apply(&self, (x, y): (f32, f32)) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// Fator de escala médio (para converter a largura do traço).
    fn scale(&self) -> f32 {
        let [a, b, c, d, ..] = self.0;
        ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) / 2.0
    }
}

#[derive(Clone, Copy, Debug)]
enum Seg {
    Move((f32, f32)),
    Line((f32, f32)),
    Cubic((f32, f32), (f32, f32), (f32, f32)),
    Close,
}

#[derive(Debug)]
struct Shape {
    segs: Vec<Seg>,
    fill: Paint,
    stroke: Paint,
    stroke_width: f32,
    evenodd: bool,
    opacity: f32,
}

#[derive(Debug)]
pub struct Svg {
    view_box: (f32, f32, f32, f32),
    pub width: f32,
    pub height: f32,
    shapes: Vec<Shape>,
}

#[derive(Clone, Copy)]
struct State {
    fill: Paint,
    stroke: Paint,
    stroke_width: f32,
    evenodd: bool,
    opacity: f32,
    matrix: Matrix,
}

fn num(s: Option<&str>) -> Option<f32> {
    s?.trim().trim_end_matches("px").parse().ok()
}

fn parse_paint(v: &str) -> Option<Paint> {
    let v = v.trim();
    match v.to_ascii_lowercase().as_str() {
        "none" | "transparent" => Some(Paint::None),
        "currentcolor" => Some(Paint::Current),
        l if l.starts_with("url(") => Some(Paint::Current),
        _ => style::parse_color(v, Color::BLACK).map(Paint::Color),
    }
}

/// Números de uma lista SVG ("1.5-2e3,4 .5.5" → 1.5, -2000, 4, 0.5, 0.5).
fn numbers(s: &str) -> Vec<f32> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
            let start = i;
            i += 1;
            let mut seen_dot = c == b'.';
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == b'.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else if (d == b'e' || d == b'E') && i + 1 < b.len() {
                    i += 1;
                    if b[i] == b'-' || b[i] == b'+' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(v) = s[start..i].parse() {
                out.push(v);
            }
        } else {
            i += 1;
        }
    }
    out
}

fn parse_transform(s: &str) -> Matrix {
    let mut m = Matrix::ID;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else { break };
        let args = numbers(&rest[open + 1..open + close]);
        let t = match (name, args.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => Matrix([*a, *b, *c, *d, *e, *f]),
            ("translate", [x]) => Matrix([1.0, 0.0, 0.0, 1.0, *x, 0.0]),
            ("translate", [x, y]) => Matrix([1.0, 0.0, 0.0, 1.0, *x, *y]),
            ("scale", [x]) => Matrix([*x, 0.0, 0.0, *x, 0.0, 0.0]),
            ("scale", [x, y]) => Matrix([*x, 0.0, 0.0, *y, 0.0, 0.0]),
            ("rotate", [a, rest @ ..]) => {
                let (sn, cs) = a.to_radians().sin_cos();
                let r = Matrix([cs, sn, -sn, cs, 0.0, 0.0]);
                match rest {
                    [cx, cy] => Matrix([1.0, 0.0, 0.0, 1.0, *cx, *cy])
                        .mul(r)
                        .mul(Matrix([1.0, 0.0, 0.0, 1.0, -cx, -cy])),
                    _ => r,
                }
            }
            _ => Matrix::ID,
        };
        m = m.mul(t);
        rest = &rest[open + close + 1..];
    }
    m
}

/// Arco elíptico (parametrização por extremos, SVG 1.1 apêndice F.6) → cúbicas.
#[allow(clippy::too_many_arguments)]
fn arc_to_cubics(
    p0: (f32, f32),
    mut rx: f32,
    mut ry: f32,
    angle: f32,
    large: bool,
    sweep: bool,
    p1: (f32, f32),
    out: &mut Vec<Seg>,
) {
    if rx == 0.0 || ry == 0.0 || p0 == p1 {
        out.push(Seg::Line(p1));
        return;
    }
    rx = rx.abs();
    ry = ry.abs();
    let (sinp, cosp) = angle.to_radians().sin_cos();
    let dx = (p0.0 - p1.0) / 2.0;
    let dy = (p0.1 - p1.1) / 2.0;
    let x1p = cosp * dx + sinp * dy;
    let y1p = -sinp * dx + cosp * dy;
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cosp * cxp - sinp * cyp + (p0.0 + p1.0) / 2.0;
    let cy = sinp * cxp + cosp * cyp + (p0.1 + p1.1) / 2.0;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        if a.is_nan() { 0.0 } else { a }
    };
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = ang(1.0, 0.0, ux, uy);
    let mut delta = ang(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }
    let n = (delta.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = delta / n as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |t: f32| {
        let (s, c) = t.sin_cos();
        let x = rx * c;
        let y = ry * s;
        (cosp * x - sinp * y + cx, sinp * x + cosp * y + cy)
    };
    let deriv = |t: f32| {
        let (s, c) = t.sin_cos();
        let x = -rx * s;
        let y = ry * c;
        (cosp * x - sinp * y, sinp * x + cosp * y)
    };
    let mut t = theta1;
    for i in 0..n {
        let t2 = t + step;
        let (a, b) = (point(t), point(t2));
        let (da, db) = (deriv(t), deriv(t2));
        let end = if i == n - 1 { p1 } else { b };
        out.push(Seg::Cubic(
            (a.0 + k * da.0, a.1 + k * da.1),
            (b.0 - k * db.0, b.1 - k * db.1),
            end,
        ));
        t = t2;
    }
}

