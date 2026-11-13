//! Renderizador de SVG próprio. Cobre o subconjunto usado por ícones e
//! logotipos: <path> (todos os comandos, inclusive arcos), <rect>, <circle>,
//! <ellipse>, <line>, <polyline>, <polygon>, <g>, `transform`, herança de
//! fill/stroke, `fill-rule`, gradientes lineares e radiais e antialias. Ainda
//! não: padrões (<pattern>), clipPath, máscaras, texto e filtros.
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
    /// currentColor
    Current,
    /// url(#id): índice em `Svg::refs` e a cor reserva (se o id não for um gradiente)
    Url(usize, Option<Color>),
}

/// Um `<linearGradient>` ou `<radialGradient>`.
#[derive(Clone, Debug, Default)]
struct Gradient {
    radial: bool,
    /// gradientUnits="userSpaceOnUse" (senão, a caixa da forma)
    user_space: Option<bool>,
    /// linear: x1 y1 x2 y2; radial: cx cy r (como frações da caixa ou em
    /// unidades do usuário); None = herdado de `href` ou o padrão
    coords: [Option<f32>; 4],
    transform: Option<Matrix>,
    stops: Vec<(f32, Color)>,
    href: Option<String>,
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
    /// Transformação do elemento (para gradientes em userSpaceOnUse).
    matrix: Matrix,
}

#[derive(Debug)]
pub struct Svg {
    view_box: (f32, f32, f32, f32),
    pub width: f32,
    pub height: f32,
    shapes: Vec<Shape>,
    /// ids citados em url(#...)
    refs: Vec<String>,
    gradients: std::collections::HashMap<String, Gradient>,
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

fn parse_paint(v: &str, refs: &mut Vec<String>) -> Option<Paint> {
    let v = v.trim();
    match v.to_ascii_lowercase().as_str() {
        "none" | "transparent" => Some(Paint::None),
        "currentcolor" => Some(Paint::Current),
        l if l.starts_with("url(") => {
            // url(#id) [cor reserva]
            let end = v.find(')')?;
            let id = v[4..end].trim().trim_matches(|c| c == '"' || c == '\'').trim_start_matches('#').to_string();
            let fallback = style::parse_color(v[end + 1..].trim(), Color::BLACK);
            let i = refs.iter().position(|r| *r == id).unwrap_or_else(|| {
                refs.push(id);
                refs.len() - 1
            });
            Some(Paint::Url(i, fallback))
        }
        _ => style::parse_color(v, Color::BLACK).map(Paint::Color),
    }
}

/// Número de um atributo de gradiente: "50%" vira 0,5.
fn frac(v: Option<&str>) -> Option<f32> {
    let v = v?.trim();
    match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().ok().map(|x| x / 100.0),
        None => v.trim_end_matches("px").parse().ok(),
    }
}

/// Números de uma lista SVG ("1.5-2e3,4 .5.5" → 1.5, -2000, 4, 0.5, 0.5).
fn numbers(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(v) = next_number(s, &mut i) {
        out.push(v);
    }
    out
}

/// Próximo número a partir de `*i` (pula separadores e lixo).
fn next_number(s: &str, i: &mut usize) -> Option<f32> {
    let b = s.as_bytes();
    while *i < b.len() {
        let c = b[*i];
        if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
            let start = *i;
            *i += 1;
            let mut seen_dot = c == b'.';
            while *i < b.len() {
                let d = b[*i];
                if d.is_ascii_digit() {
                    *i += 1;
                } else if d == b'.' && !seen_dot {
                    seen_dot = true;
                    *i += 1;
                } else if (d == b'e' || d == b'E') && *i + 1 < b.len() {
                    *i += 1;
                    if b[*i] == b'-' || b[*i] == b'+' {
                        *i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(v) = s[start..*i].parse() {
                return Some(v);
            }
        } else {
            *i += 1;
        }
    }
    None
}

/// Argumentos de um arco: em cada grupo de 7, o 4º e o 5º são bandeiras de um
/// caractere só, que podem vir grudadas no número seguinte ("a1 1 0 000 2" =
/// 1, 1, 0, 0, 0, 0, 2).
fn arc_numbers(s: &str) -> Vec<f32> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        if matches!(out.len() % 7, 3 | 4) {
            while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
                i += 1;
            }
            match b.get(i) {
                Some(b'0') => out.push(0.0),
                Some(b'1') => out.push(1.0),
                _ => break,
            }
            i += 1;
        } else {
            match next_number(s, &mut i) {
                Some(v) => out.push(v),
                None => break,
            }
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

/// Dados de <path d="...">, em coordenadas absolutas.
/// Dados de caminho SVG (`d`) no formato compacto do canvas: 0 x y (move),
/// 1 x y (linha), 2 c1x c1y c2x c2y x y (cúbica), 3 (fecha).
pub fn path_commands(d: &str) -> Vec<f64> {
    let mut out = Vec::new();
    for s in parse_path(d) {
        match s {
            Seg::Move((x, y)) => out.extend([0.0, x as f64, y as f64]),
            Seg::Line((x, y)) => out.extend([1.0, x as f64, y as f64]),
            Seg::Cubic(a, b, c) => out.extend([2.0, a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64, c.0 as f64, c.1 as f64]),
            Seg::Close => out.push(3.0),
        }
    }
    out
}

fn parse_path(d: &str) -> Vec<Seg> {
    // separa em (comando, números)
    let mut cmds: Vec<(char, Vec<f32>)> = Vec::new();
    let mut start = None;
    let args = |cmd: char, s: &str| if matches!(cmd, 'a' | 'A') { arc_numbers(s) } else { numbers(s) };
    for (i, c) in d.char_indices() {
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            if let Some((s, cmd)) = start {
                cmds.push((cmd, args(cmd, &d[s..i])));
            }
            start = Some((i + 1, c));
        }
    }
    if let Some((s, cmd)) = start {
        cmds.push((cmd, args(cmd, &d[s..])));
    }

    let mut out = Vec::new();
    let mut cur = (0.0f32, 0.0f32);
    let mut start_pt = cur;
    let mut last_ctrl: Option<(f32, f32)> = None; // para S/T
    let mut last_quad: Option<(f32, f32)> = None;
    for (cmd, args) in cmds {
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        let off = |p: (f32, f32), cur: (f32, f32)| if rel { (p.0 + cur.0, p.1 + cur.1) } else { p };
        let arity = match up {
            'M' | 'L' | 'T' => 2,
            'H' | 'V' => 1,
            'C' => 6,
            'S' | 'Q' => 4,
            'A' => 7,
            _ => 0,
        };
        if up == 'Z' {
            out.push(Seg::Close);
            cur = start_pt;
            last_ctrl = None;
            last_quad = None;
            continue;
        }
        if arity == 0 || args.len() < arity {
            continue;
        }
        for (k, a) in args.chunks_exact(arity).enumerate() {
            let mut ctrl = None;
            let mut quad = None;
            match up {
                'M' => {
                    let p = off((a[0], a[1]), cur);
                    if k == 0 {
                        out.push(Seg::Move(p));
                        start_pt = p;
                    } else {
                        out.push(Seg::Line(p)); // pares extras de M viram L
                    }
                    cur = p;
                }
                'L' => {
                    cur = off((a[0], a[1]), cur);
                    out.push(Seg::Line(cur));
                }
                'H' => {
                    cur = (if rel { cur.0 + a[0] } else { a[0] }, cur.1);
                    out.push(Seg::Line(cur));
                }
                'V' => {
                    cur = (cur.0, if rel { cur.1 + a[0] } else { a[0] });
                    out.push(Seg::Line(cur));
                }
                'C' => {
                    let c1 = off((a[0], a[1]), cur);
                    let c2 = off((a[2], a[3]), cur);
                    let p = off((a[4], a[5]), cur);
                    out.push(Seg::Cubic(c1, c2, p));
                    ctrl = Some(c2);
                    cur = p;
                }
                'S' => {
                    let c1 = last_ctrl.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1));
                    let c2 = off((a[0], a[1]), cur);
                    let p = off((a[2], a[3]), cur);
                    out.push(Seg::Cubic(c1, c2, p));
                    ctrl = Some(c2);
                    cur = p;
                }
                'Q' | 'T' => {
                    let q = if up == 'Q' {
                        off((a[0], a[1]), cur)
                    } else {
                        last_quad.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1))
                    };
                    let p = if up == 'Q' { off((a[2], a[3]), cur) } else { off((a[0], a[1]), cur) };
                    // quadrática → cúbica
                    let c1 = (cur.0 + 2.0 / 3.0 * (q.0 - cur.0), cur.1 + 2.0 / 3.0 * (q.1 - cur.1));
                    let c2 = (p.0 + 2.0 / 3.0 * (q.0 - p.0), p.1 + 2.0 / 3.0 * (q.1 - p.1));
                    out.push(Seg::Cubic(c1, c2, p));
                    quad = Some(q);
                    cur = p;
                }
                'A' => {
                    let p = off((a[5], a[6]), cur);
                    arc_to_cubics(cur, a[0], a[1], a[2], a[3] != 0.0, a[4] != 0.0, p, &mut out);
                    cur = p;
                }
                _ => {}
            }
            last_ctrl = ctrl;
            last_quad = quad;
        }
    }
    out
}

/// Elipse como 4 cúbicas (k = 0,5523).
fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<Seg> {
    let k = 0.552_284_8;
    vec![
        Seg::Move((cx + rx, cy)),
        Seg::Cubic((cx + rx, cy + k * ry), (cx + k * rx, cy + ry), (cx, cy + ry)),
        Seg::Cubic((cx - k * rx, cy + ry), (cx - rx, cy + k * ry), (cx - rx, cy)),
        Seg::Cubic((cx - rx, cy - k * ry), (cx - k * rx, cy - ry), (cx, cy - ry)),
        Seg::Cubic((cx + k * rx, cy - ry), (cx + rx, cy - k * ry), (cx + rx, cy)),
        Seg::Close,
    ]
}

fn round_rect(x: f32, y: f32, w: f32, h: f32, rx: f32, ry: f32) -> Vec<Seg> {
    let rx = rx.min(w / 2.0);
    let ry = ry.min(h / 2.0);
    if rx <= 0.0 || ry <= 0.0 {
        return vec![
            Seg::Move((x, y)),
            Seg::Line((x + w, y)),
            Seg::Line((x + w, y + h)),
            Seg::Line((x, y + h)),
            Seg::Close,
        ];
    }
    let k = 0.552_284_8;
    let (kx, ky) = (k * rx, k * ry);
    vec![
        Seg::Move((x + rx, y)),
        Seg::Line((x + w - rx, y)),
        Seg::Cubic((x + w - rx + kx, y), (x + w, y + ry - ky), (x + w, y + ry)),
        Seg::Line((x + w, y + h - ry)),
        Seg::Cubic((x + w, y + h - ry + ky), (x + w - rx + kx, y + h), (x + w - rx, y + h)),
        Seg::Line((x + rx, y + h)),
        Seg::Cubic((x + rx - kx, y + h), (x, y + h - ry + ky), (x, y + h - ry)),
        Seg::Line((x, y + ry)),
        Seg::Cubic((x, y + ry - ky), (x + rx - kx, y), (x + rx, y)),
        Seg::Close,
    ]
}

/// Lê um documento SVG.
pub fn parse(src: &str) -> Option<Svg> {
    let mut svg: Option<Svg> = None;
    let mut stack: Vec<State> = Vec::new();
    let mut skip_depth = 0usize; // dentro de <defs>, <clipPath>...
    let mut refs: Vec<String> = Vec::new();
    let mut gradients: std::collections::HashMap<String, Gradient> = Default::default();
    // gradiente aberto (recebe as <stop>)
    let mut open_grad: Option<String> = None;
    let root = State {
        fill: Paint::Color(Color::BLACK),
        stroke: Paint::None,
        stroke_width: 1.0,
        evenodd: false,
        opacity: 1.0,
        matrix: Matrix::ID,
    };

    for tok in html::tokenize(src) {
        match tok {
            Token::StartTag {
                name,
                attrs,
                self_closing,
            } => {
                let attr = |n: &str| attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
                // gradientes (em qualquer lugar, normalmente dentro de <defs>)
                if name == "lineargradient" || name == "radialgradient" {
                    let radial = name == "radialgradient";
                    let keys: [&str; 4] = if radial { ["cx", "cy", "r", ""] } else { ["x1", "y1", "x2", "y2"] };
                    let g = Gradient {
                        radial,
                        user_space: attr("gradientunits").map(|u| u.trim().eq_ignore_ascii_case("userSpaceOnUse")),
                        coords: keys.map(|k| if k.is_empty() { None } else { frac(attr(k)) }),
                        transform: attr("gradienttransform").map(parse_transform),
                        stops: Vec::new(),
                        href: attr("href").or(attr("xlink:href")).map(|h| h.trim().trim_start_matches('#').to_string()),
                    };
                    let id = attr("id").unwrap_or("").to_string();
                    gradients.insert(id.clone(), g);
                    open_grad = if self_closing { None } else { Some(id) };
                    if !self_closing {
                        skip_depth += 1;
                    }
                    continue;
                }
                if name == "stop" {
                    if let Some(g) = open_grad.as_ref().and_then(|id| gradients.get_mut(id)) {
                        let mut props: Vec<(String, String)> = attrs.clone();
                        if let Some(s) = attr("style") {
                            for d in css::parse_declarations(s) {
                                props.push((d.name, d.value));
                            }
                        }
                        let get = |k: &str| props.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
                        let offset = frac(get("offset")).unwrap_or(0.0).clamp(0.0, 1.0);
                        let mut c = get("stop-color").and_then(|v| style::parse_color(v, Color::BLACK)).unwrap_or(Color::BLACK);
                        let op = get("stop-opacity").and_then(|v| frac(Some(v))).unwrap_or(1.0).clamp(0.0, 1.0);
                        c.a = (c.a as f32 * op).round() as u8;
                        // offsets não diminuem
                        let offset = g.stops.last().map_or(offset, |l| offset.max(l.0));
                        g.stops.push((offset, c));
                    }
                    if !self_closing {
                        skip_depth += 1;
                    }
                    continue;
                }
                if skip_depth > 0 {
                    if !self_closing {
                        skip_depth += 1;
                    }
                    continue;
                }
                if matches!(
                    name.as_str(),
                    "defs" | "clippath" | "mask" | "symbol" | "lineargradient" | "radialgradient" | "pattern"
                        | "filter" | "style" | "title" | "desc" | "metadata"
                ) {
                    if !self_closing {
                        skip_depth = 1;
                    }
                    continue;
                }
                let parent = stack.last().copied().unwrap_or(root);
                let mut st = parent;
                // atributos de apresentação, depois o atributo style
                let mut props: Vec<(String, String)> = attrs.clone();
                if let Some(s) = attr("style") {
                    for d in css::parse_declarations(s) {
                        props.push((d.name, d.value));
                    }
                }
                for (k, v) in &props {
                    match k.as_str() {
                        "fill" => {
                            if let Some(p) = parse_paint(v, &mut refs) {
                                st.fill = p
                            }
                        }
                        "stroke" => {
                            if let Some(p) = parse_paint(v, &mut refs) {
                                st.stroke = p
                            }
                        }
                        "stroke-width" => {
                            if let Some(w) = num(Some(v)) {
                                st.stroke_width = w
                            }
                        }
                        "fill-rule" => st.evenodd = v.trim() == "evenodd",
                        "opacity" => st.opacity *= num(Some(v)).unwrap_or(1.0),
                        "transform" => st.matrix = parent.matrix.mul(parse_transform(v)),
                        _ => {}
                    }
                }

                let n = |k: &str| num(attr(k));
                let segs = match name.as_str() {
                    "svg" => {
                        if svg.is_none() {
                            let vb = attr("viewbox").map(numbers).filter(|v| v.len() == 4);
                            let w = n("width").or(vb.as_ref().map(|v| v[2])).unwrap_or(24.0);
                            let h = n("height").or(vb.as_ref().map(|v| v[3])).unwrap_or(24.0);
                            let view_box = vb.map_or((0.0, 0.0, w, h), |v| (v[0], v[1], v[2], v[3]));
                            svg = Some(Svg {
                                view_box,
                                width: w,
                                height: h,
                                shapes: Vec::new(),
                                refs: Vec::new(),
                                gradients: Default::default(),
                            });
                        }
                        None
                    }
                    "path" => attr("d").map(parse_path),
                    "rect" => {
                        let (w, h) = (n("width").unwrap_or(0.0), n("height").unwrap_or(0.0));
                        let rx = n("rx").or(n("ry")).unwrap_or(0.0);
                        let ry = n("ry").or(n("rx")).unwrap_or(0.0);
                        Some(round_rect(n("x").unwrap_or(0.0), n("y").unwrap_or(0.0), w, h, rx, ry))
                    }
                    "circle" => {
                        let r = n("r").unwrap_or(0.0);
                        Some(ellipse(n("cx").unwrap_or(0.0), n("cy").unwrap_or(0.0), r, r))
                    }
                    "ellipse" => Some(ellipse(
                        n("cx").unwrap_or(0.0),
                        n("cy").unwrap_or(0.0),
                        n("rx").unwrap_or(0.0),
                        n("ry").unwrap_or(0.0),
                    )),
                    "line" => Some(vec![
                        Seg::Move((n("x1").unwrap_or(0.0), n("y1").unwrap_or(0.0))),
                        Seg::Line((n("x2").unwrap_or(0.0), n("y2").unwrap_or(0.0))),
                    ]),
                    "polyline" | "polygon" => attr("points").map(|p| {
                        let v = numbers(p);
                        let mut segs: Vec<Seg> = v
                            .chunks_exact(2)
                            .enumerate()
                            .map(|(i, c)| if i == 0 { Seg::Move((c[0], c[1])) } else { Seg::Line((c[0], c[1])) })
                            .collect();
                        if name == "polygon" {
                            segs.push(Seg::Close);
                        }
                        segs
                    }),
                    _ => None,
                };
                if let (Some(segs), Some(svg)) = (segs, svg.as_mut()) {
                    let m = st.matrix;
                    let segs = segs
                        .into_iter()
                        .map(|s| match s {
                            Seg::Move(p) => Seg::Move(m.apply(p)),
                            Seg::Line(p) => Seg::Line(m.apply(p)),
                            Seg::Cubic(a, b, c) => Seg::Cubic(m.apply(a), m.apply(b), m.apply(c)),
                            Seg::Close => Seg::Close,
                        })
                        .collect();
                    // linhas não têm preenchimento
                    let fill = if matches!(name.as_str(), "line" | "polyline") { Paint::None } else { st.fill };
                    svg.shapes.push(Shape {
                        segs,
                        fill,
                        stroke: st.stroke,
                        stroke_width: st.stroke_width * m.scale(),
                        evenodd: st.evenodd,
                        opacity: st.opacity,
                        matrix: m,
                    });
                }
                if !self_closing {
                    stack.push(st);
                }
            }
            Token::EndTag { name, .. } => {
                if name == "lineargradient" || name == "radialgradient" {
                    open_grad = None;
                }
                if skip_depth > 0 {
                    skip_depth -= 1;
                } else {
                    stack.pop();
                }
            }
            _ => {}
        }
    }
    let mut svg = svg?;
    svg.refs = refs;
    svg.gradients = gradients;
    Some(svg)
}

impl Matrix {
    fn invert(&self) -> Option<Matrix> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Some(Matrix([ia, ib, ic, id, -(ia * e + ic * f), -(ib * e + id * f)]))
    }
}

/// Gradiente pronto para pintar: do pixel para o espaço do gradiente e as paradas.
struct GradPaint {
    radial: bool,
    inv: Matrix,
    c: [f32; 4],
    stops: Vec<(f32, Color)>,
}

impl GradPaint {
    /// Cor (RGBA de 0 a 1, sem pré-multiplicar) no centro do pixel.
    fn at(&self, x: f32, y: f32) -> [f32; 4] {
        let (gx, gy) = self.inv.apply((x, y));
        let t = if self.radial {
            let r = self.c[2].max(1e-6);
            ((gx - self.c[0]).powi(2) + (gy - self.c[1]).powi(2)).sqrt() / r
        } else {
            let (dx, dy) = (self.c[2] - self.c[0], self.c[3] - self.c[1]);
            let len2 = (dx * dx + dy * dy).max(1e-12);
            ((gx - self.c[0]) * dx + (gy - self.c[1]) * dy) / len2
        }
        .clamp(0.0, 1.0);
        let s = &self.stops;
        let col = |c: Color| [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, c.a as f32 / 255.0];
        if t <= s[0].0 {
            return col(s[0].1);
        }
        for w in s.windows(2) {
            if t <= w[1].0 {
                let span = (w[1].0 - w[0].0).max(1e-6);
                let k = (t - w[0].0) / span;
                let (a, b) = (col(w[0].1), col(w[1].1));
                return [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * k);
            }
        }
        col(s[s.len() - 1].1)
    }
}

impl Svg {
    /// O gradiente de um url(#id), com os atributos e paradas herdados por
    /// href e o mapeamento para os pixels da forma.
    fn gradient(&self, i: usize, segs: &[Seg], shape_m: Matrix, view: Matrix) -> Option<GradPaint> {
        let id = self.refs.get(i)?;
        let mut g = self.gradients.get(id)?.clone();
        // herança por href (até 8 níveis)
        let mut next = g.href.clone();
        for _ in 0..8 {
            let Some(h) = next else { break };
            let Some(p) = self.gradients.get(&h) else { break };
            if g.stops.is_empty() {
                g.stops = p.stops.clone();
            }
            if p.radial == g.radial {
                for k in 0..4 {
                    g.coords[k] = g.coords[k].or(p.coords[k]);
                }
            }
            g.user_space = g.user_space.or(p.user_space);
            g.transform = g.transform.or(p.transform);
            next = p.href.clone();
        }
        if g.stops.is_empty() {
            return None;
        }
        let user = g.user_space.unwrap_or(false);
        let c = if g.radial {
            [g.coords[0].unwrap_or(0.5), g.coords[1].unwrap_or(0.5), g.coords[2].unwrap_or(0.5), 0.0]
        } else {
            [g.coords[0].unwrap_or(0.0), g.coords[1].unwrap_or(0.0), g.coords[2].unwrap_or(1.0), g.coords[3].unwrap_or(0.0)]
        };
        // espaço do gradiente → espaço do usuário da raiz → pixels
        let base = if user {
            shape_m
        } else {
            // a caixa da forma (já no espaço da raiz)
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for s in segs {
                let pts: &[(f32, f32)] = match s {
                    Seg::Move(p) | Seg::Line(p) => std::slice::from_ref(p),
                    Seg::Cubic(a, b, c) => &[*a, *b, *c],
                    Seg::Close => &[],
                };
                for &(x, y) in pts {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
            }
            if x1 <= x0 || y1 <= y0 {
                return None;
            }
            Matrix([x1 - x0, 0.0, 0.0, y1 - y0, x0, y0])
        };
        let m = view.mul(base).mul(g.transform.unwrap_or(Matrix::ID));
        Some(GradPaint { radial: g.radial, inv: m.invert()?, c, stops: g.stops })
    }
}

/// Achata as curvas em polilinhas (já em pixels). Devolve (pontos, fechado).
fn flatten(segs: &[Seg], map: impl Fn((f32, f32)) -> (f32, f32)) -> Vec<(Vec<(f32, f32)>, bool)> {
    let mut out: Vec<(Vec<(f32, f32)>, bool)> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut start = (0.0, 0.0);
    let mut last = (0.0, 0.0);
    for s in segs {
        match *s {
            Seg::Move(p) => {
                if cur.len() > 1 {
                    out.push((std::mem::take(&mut cur), false));
                }
                cur.clear();
                last = map(p);
                start = last;
                cur.push(last);
            }
            Seg::Line(p) => {
                if cur.is_empty() {
                    cur.push(last);
                }
                last = map(p);
                cur.push(last);
            }
            Seg::Cubic(c1, c2, p) => {
                if cur.is_empty() {
                    cur.push(last);
                }
                let (p0, p1, p2, p3) = (last, map(c1), map(c2), map(p));
                let d = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
                let len = d(p0, p1) + d(p1, p2) + d(p2, p3);
                let n = (len / 1.5).ceil().clamp(2.0, 96.0) as usize;
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let mt = 1.0 - t;
                    let (a, b, c, e) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
                    cur.push((
                        a * p0.0 + b * p1.0 + c * p2.0 + e * p3.0,
                        a * p0.1 + b * p1.1 + c * p2.1 + e * p3.1,
                    ));
                }
                last = map(p);
            }
            Seg::Close => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), true));
                }
                last = start;
            }
        }
    }
    if cur.len() > 1 {
        out.push((cur, false));
    }
    out
}

/// Cobertura do preenchimento por varredura (4 sub-linhas por pixel, cobertura
/// horizontal exata).
fn fill_coverage(polys: &[(Vec<(f32, f32)>, bool)], evenodd: bool, w: usize, h: usize, cov: &mut [f32]) {
    const SUB: usize = 4;
    let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();
    for (pts, _) in polys {
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()]; // todo subcaminho é fechado para preencher
            if a.1 != b.1 {
                edges.push((a.0, a.1, b.0, b.1));
            }
        }
    }
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for py in 0..h {
        for sub in 0..SUB {
            let ys = py as f32 + (sub as f32 + 0.5) / SUB as f32;
            xs.clear();
            for &(x0, y0, x1, y1) in &edges {
                let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                if ys >= lo && ys < hi {
                    let x = x0 + (ys - y0) * (x1 - x0) / (y1 - y0);
                    xs.push((x, if y1 > y0 { 1 } else { -1 }));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            for k in 0..xs.len() {
                wind += xs[k].1;
                let inside = if evenodd { wind % 2 != 0 } else { wind != 0 };
                if inside && k + 1 < xs.len() {
                    let (xa, xb) = (xs[k].0.max(0.0), xs[k + 1].0.min(w as f32));
                    if xb <= xa {
                        continue;
                    }
                    let row = py * w;
                    let mut ix = xa.floor() as usize;
                    while (ix as f32) < xb && ix < w {
                        let overlap = (xb.min(ix as f32 + 1.0) - xa.max(ix as f32)).max(0.0);
                        cov[row + ix] += overlap / SUB as f32;
                        ix += 1;
                    }
                }
            }
        }
    }
}

/// Cobertura do traço: distância de cada pixel ao segmento mais próximo.
fn stroke_coverage(polys: &[(Vec<(f32, f32)>, bool)], width: f32, w: usize, h: usize, cov: &mut [f32]) {
    let hw = (width / 2.0).max(0.35);
    for (pts, closed) in polys {
        let n = pts.len();
        let segs = if *closed { n } else { n - 1 };
        for i in 0..segs {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            let x0 = ((a.0.min(b.0) - hw - 1.0).floor().max(0.0)) as usize;
            let x1 = ((a.0.max(b.0) + hw + 1.0).ceil().min(w as f32)) as usize;
            let y0 = ((a.1.min(b.1) - hw - 1.0).floor().max(0.0)) as usize;
            let y1 = ((a.1.max(b.1) + hw + 1.0).ceil().min(h as f32)) as usize;
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len2 = dx * dx + dy * dy;
            for py in y0..y1 {
                for px in x0..x1 {
                    let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
                    let t = if len2 > 0.0 {
                        (((cx - a.0) * dx + (cy - a.1) * dy) / len2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let (qx, qy) = (a.0 + t * dx - cx, a.1 + t * dy - cy);
                    let d = (qx * qx + qy * qy).sqrt();
                    let c = (hw + 0.5 - d).clamp(0.0, 1.0);
                    let i = py * w + px;
                    if c > cov[i] {
                        cov[i] = c;
                    }
                }
            }
        }
    }
}

impl Svg {
    /// Rasteriza em `w`×`h` pixels. Com `color`, toda tinta vira essa cor
    /// (modo ícone monocromático).
    pub fn render(&self, w: u32, h: u32, color: Option<Color>) -> ImageData {
        self.render_with(w, h, color, Color::BLACK)
    }

    /// Como `render`, com a cor de `currentColor` (a cor CSS do elemento).
    pub fn render_with(&self, w: u32, h: u32, color: Option<Color>, current: Color) -> ImageData {
        let (wu, hu) = (w as usize, h as usize);
        let (vx, vy, vw, vh) = self.view_box;
        let s = (w as f32 / vw).min(h as f32 / vh);
        let tx = (w as f32 - vw * s) / 2.0 - vx * s;
        let ty = (h as f32 - vh * s) / 2.0 - vy * s;
        let map = |(x, y): (f32, f32)| (x * s + tx, y * s + ty);
        let view = Matrix([s, 0.0, 0.0, s, tx, ty]);

        // RGBA pré-multiplicado em f32
        let mut acc = vec![[0.0f32; 4]; wu * hu];
        let mut cov = vec![0.0f32; wu * hu];
        for shape in &self.shapes {
            let polys = flatten(&shape.segs, map);
            for (paint, is_fill) in [(shape.fill, true), (shape.stroke, false)] {
                // url(#id): gradiente; sem ele, a cor reserva ou nada
                let mut grad = None;
                let c = match (paint, color) {
                    (Paint::None, _) => continue,
                    (_, Some(c)) => c,
                    (Paint::Color(c), None) => c,
                    (Paint::Current, None) => current,
                    (Paint::Url(i, fallback), None) => match self.gradient(i, &shape.segs, shape.matrix, view) {
                        Some(g) => {
                            grad = Some(g);
                            Color::BLACK
                        }
                        None => match fallback {
                            Some(c) => c,
                            None => continue,
                        },
                    },
                };
                cov.iter_mut().for_each(|v| *v = 0.0);
                if is_fill {
                    fill_coverage(&polys, shape.evenodd, wu, hu, &mut cov);
                } else {
                    stroke_coverage(&polys, shape.stroke_width * s, wu, hu, &mut cov);
                }
                let alpha = c.a as f32 / 255.0 * shape.opacity;
                let col = [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0];
                for (i, (dst, &cv)) in acc.iter_mut().zip(cov.iter()).enumerate() {
                    if cv <= 0.0 {
                        continue;
                    }
                    let (col, alpha) = match &grad {
                        Some(g) => {
                            let p = g.at((i % wu) as f32 + 0.5, (i / wu) as f32 + 0.5);
                            ([p[0], p[1], p[2]], p[3] * shape.opacity)
                        }
                        None => (col, alpha),
                    };
                    let a = cv.min(1.0) * alpha;
                    if a <= 0.0 {
                        continue;
                    }
                    for k in 0..3 {
                        dst[k] = col[k] * a + dst[k] * (1.0 - a);
                    }
                    dst[3] = a + dst[3] * (1.0 - a);
                }
            }
        }
        let mut rgba = Vec::with_capacity(wu * hu * 4);
        for p in acc {
            let a = p[3];
            let un = |v: f32| if a > 0.0 { (v / a * 255.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
            rgba.extend_from_slice(&[un(p[0]), un(p[1]), un(p[2]), (a * 255.0).round() as u8]);
        }
        ImageData {
            width: w,
            height: h,
            rgba,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeros_svg() {
        assert_eq!(numbers("1.5-2e1,4 .5.5"), vec![1.5, -20.0, 4.0, 0.5, 0.5]);
    }

    #[test]
    fn bandeiras_do_arco_grudadas() {
        // "a1 1 0 000 2" = rx 1, ry 1, rotação 0, bandeiras 0 e 0, ponto (0, 2)
        assert_eq!(arc_numbers("1 1 0 000 2"), vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 2.0]);
        assert_eq!(arc_numbers("9 9 0 105.641 16.01"), vec![9.0, 9.0, 0.0, 1.0, 0.0, 5.641, 16.01]);
        assert_eq!(arc_numbers("1 1 0 0,1 2 0 1 1 0 1 1 3 3"), vec![1.0, 1.0, 0.0, 0.0, 1.0, 2.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 3.0, 3.0]);
        // a lupa do YouTube termina no ponto certo
        let segs = parse_path("M11 2a9 9 0 105.641 16.01");
        let Some(Seg::Cubic(_, _, end)) = segs.last() else { panic!() };
        assert!((end.0 - 16.641).abs() < 0.01 && (end.1 - 18.01).abs() < 0.01, "{end:?}");
    }

    #[test]
    fn caminho_relativo_e_arco() {
        let segs = parse_path("m10 10 h5 v5 z M0 0 a5 5 0 0 1 10 0");
        assert!(matches!(segs[0], Seg::Move((10.0, 10.0))));
        assert!(matches!(segs[2], Seg::Line((15.0, 15.0))));
        assert!(segs.iter().filter(|s| matches!(s, Seg::Cubic(..))).count() >= 2);
    }

    #[test]
    fn quadrado_preenchido_e_traco() {
        let svg = parse(
            r#"<svg viewBox="0 0 10 10"><rect x="0" y="0" width="5" height="10" fill="red"/>
               <path d="M7 0 V10" stroke="blue" stroke-width="2" fill="none"/></svg>"#,
        )
        .unwrap();
        let img = svg.render(10, 10, None);
        let px = |x: usize, y: usize| &img.rgba[(y * 10 + x) * 4..(y * 10 + x) * 4 + 4];
        assert_eq!(px(2, 5), &[255, 0, 0, 255]);
        assert_eq!(px(7, 5)[2], 255);
        assert_eq!(px(9, 5)[3], 0);
    }

    #[test]
    fn gradiente_linear_e_url_sem_servidor() {
        let svg = parse(
            r##"<svg width="10" height="10" viewBox="0 0 10 10"><defs><linearGradient id="g"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient></defs><rect width="10" height="5" fill="url(#g)"/><rect y="5" width="5" height="5" fill="url(#nada)"/><rect x="5" y="5" width="5" height="5" fill="url(#nada) #0f0"/></svg>"##,
        )
        .unwrap();
        let img = svg.render(10, 10, None);
        let px = |x: usize, y: usize| {
            let i = (y * 10 + x) * 4;
            (img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3])
        };
        // vermelho à esquerda, azul à direita
        let (l, r) = (px(0, 2), px(9, 2));
        assert!(l.0 > 200 && l.2 < 40, "{l:?}");
        assert!(r.2 > 200 && r.0 < 40, "{r:?}");
        // url(#nada) sem reserva não pinta; com reserva, pinta a reserva
        assert_eq!(px(2, 7).3, 0);
        assert_eq!(px(7, 7), (0, 255, 0, 255));
    }

}
