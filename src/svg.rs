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

