//! Pintura em software: executa a lista de exibição (retângulos, cantos
//! arredondados, texto, imagens, recortes) num buffer de pixels 0x00RRGGBB.
//! O mesmo Canvas pinta as páginas e a interface do navegador.

use std::path::Path;
use std::sync::Arc;

use crate::fonts::{FontId, Fonts};
use crate::layout::Rect;
use crate::style::Color;

/// Imagem decodificada (RGBA, alfa não pré-multiplicado).
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageData({}x{})", self.width, self.height)
    }
}

#[derive(Debug, Clone)]
pub enum DisplayItem {
    Rect {
        rect: Rect,
        color: Color,
    },
    RoundRect {
        rect: Rect,
        radius: f32,
        color: Color,
    },
    Text {
        x: f32,
        baseline: f32,
        text: String,
        font: FontId,
        size: f32,
        color: Color,
    },
    Image {
        rect: Rect,
        image: Arc<ImageData>,
    },
    /// Borda de largura única com cantos arredondados.
    RoundBorder {
        rect: Rect,
        radius: f32,
        width: f32,
        color: Color,
    },
    /// Gradiente linear (ângulo em graus, paradas de 0 a 1).
    Gradient {
        rect: Rect,
        radius: f32,
        angle: f32,
        stops: Arc<Vec<(Color, f32)>>,
    },
    /// Imagem de fundo: `tile` é a posição e o tamanho de uma cópia; com
    /// repetição, as cópias cobrem `rect`.
    BgImage {
        rect: Rect,
        image: Arc<ImageData>,
        tile: Rect,
        repeat: (bool, bool),
        /// Pinta só o formato da imagem com esta cor (mask-image).
        tint: Option<Color>,
    },
    PushClip(Rect),
    PopClip,
    /// Começo do conteúdo de um elemento posicionado (relative/sticky): o
    /// layout separa o trecho numa camada pintada na ordem da árvore.
    /// `z: None` = z-index auto. Nunca chegam à pintura.
    LayerStart { z: Option<i32>, order: usize },
    LayerEnd,
    /// Área de um `<video>`: o pintor põe ali o quadro atual (sem refazer o layout).
    Video { rect: Rect, node: usize },
    /// Área de um `<canvas>`: a imagem dele (do mapa de quadros) esticada.
    Canvas { rect: Rect, node: usize },
    /// Trecho com `position: fixed`: não acompanha a rolagem (o pintor
    /// desfaz o deslocamento da rolagem entre as marcas).
    FixedStart,
    FixedEnd,
    /// Trecho transformado (rotate/scale/skew/matrix): `m` é a matriz em
    /// torno de `origin` (coordenadas do documento). O pintor desenha o
    /// trecho à parte e o aplica transformado.
    TransformStart { m: [f32; 6], origin: (f32, f32) },
    TransformEnd,
    /// Sombra (box-shadow): retângulo já deslocado e espalhado.
    Shadow { rect: Rect, radius: f32, blur: f32, color: Color },
    /// Área do elemento que recebe o mouse, na ordem de pintura. Só existe
    /// durante o layout (vira `Page::hits`); nunca chega à pintura.
    Hit { rect: Rect, node: usize },
}

/// Coordenadas CSS → pixels do dispositivo: `p * scale + d`.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub scale: f32,
    pub dx: f32,
    pub dy: f32,
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        scale: 1.0,
        dx: 0.0,
        dy: 0.0,
    };

    pub fn rect(&self, r: Rect) -> Rect {
        Rect {
            x: r.x * self.scale + self.dx,
            y: r.y * self.scale + self.dy,
            w: r.w * self.scale,
            h: r.h * self.scale,
        }
    }
}

/// Retângulo inteiro [x0, x1) × [y0, y1) em pixels.
#[derive(Clone, Copy)]
struct Clip {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Clip {
    fn intersect(self, o: Clip) -> Clip {
        Clip {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }

