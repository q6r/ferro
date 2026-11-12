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

    fn from_rect(r: Rect) -> Clip {
        Clip {
            x0: r.x.floor() as i32,
            y0: r.y.floor() as i32,
            x1: (r.x + r.w).ceil() as i32,
            y1: (r.y + r.h).ceil() as i32,
        }
    }

    fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
}

/// Distância (com sinal) do ponto à borda de um retângulo arredondado.
#[inline]
fn rr_sdf(px: f32, py: f32, r: Rect, rad: f32) -> f32 {
    let qx = (px - (r.x + r.w / 2.0)).abs() - (r.w / 2.0 - rad);
    let qy = (py - (r.y + r.h / 2.0)).abs() - (r.h / 2.0 - rad);
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - rad
}

#[inline]
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: l(a.a, b.a),
    }
}

#[inline]
fn pack(c: Color) -> u32 {
    (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32
}

#[inline]
fn mix(dst: u32, r: u32, g: u32, b: u32, a: u32) -> u32 {
    let inv = 255 - a;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    let nr = (r * a + dr * inv + 127) / 255;
    let ng = (g * a + dg * inv + 127) / 255;
    let nb = (b * a + db * inv + 127) / 255;
    nr << 16 | ng << 8 | nb
}

#[derive(Default)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// Um u32 por pixel, 0x00RRGGBB (o formato que a janela espera).
    pub pixels: Vec<u32>,
    /// Quadro atual de cada `<video>` (pelo nó).
    pub videos: std::collections::HashMap<usize, Arc<ImageData>>,
    /// Rolagem atual em pixels do dispositivo: os trechos `fixed` são
    /// pintados como se ela fosse zero.
    pub fixed_shift: f32,
    /// `<video controls>` parados: ganham o botão de tocar por cima.
    pub paused: std::collections::HashSet<usize>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, bg: Color) -> Canvas {
        Canvas {
            width,
            height,
            videos: Default::default(),
            fixed_shift: 0.0,
            paused: Default::default(),
            pixels: vec![pack(bg); width * height],
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if width != self.width || height != self.height {
            self.width = width;
            self.height = height;
            self.pixels = vec![0; width * height];
        }
    }

    pub fn clear(&mut self, color: Color) {
        self.pixels.fill(pack(color));
    }

    fn full(&self) -> Clip {
        Clip {
            x0: 0,
            y0: 0,
            x1: self.width as i32,
            y1: self.height as i32,
        }
    }

    fn clip(&self, clip: Option<Rect>) -> Clip {
        match clip {
            Some(r) => self.full().intersect(Clip::from_rect(r)),
            None => self.full(),
        }
    }

    /// Pinta a lista de exibição (modo linha de comando).
    pub fn paint(&mut self, items: &[DisplayItem], fonts: &Fonts, scroll_y: f32) {
        let t = Transform {
            scale: 1.0,
            dx: 0.0,
            dy: -scroll_y,
        };
        self.paint_list(items, fonts, t, None);
    }

    /// Pinta a lista de exibição com uma transformação, dentro de `clip`
    /// (em pixels do dispositivo).
    pub fn paint_list(&mut self, items: &[DisplayItem], fonts: &Fonts, t0: Transform, clip: Option<Rect>) {
        let base = self.clip(clip);
        let mut clips = vec![base];
        let mut t = t0;
        let mut fixed = 0u32;
        let mut idx = 0;
        while idx < items.len() {
            let item = &items[idx];
            idx += 1;
            let clip = *clips.last().unwrap();
            match item {
                DisplayItem::TransformStart { m, origin } => {
                    // o trecho até o TransformEnd correspondente
                    let start = idx;
                    let mut depth = 1;
                    while idx < items.len() {
                        match items[idx] {
                            DisplayItem::TransformStart { .. } => depth += 1,
                            DisplayItem::TransformEnd => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        idx += 1;
                    }
                    let sub = &items[start..idx.min(items.len())];
                    idx += 1;
                    self.paint_transformed(sub, fonts, t, *m, *origin, clip);
                }
                DisplayItem::TransformEnd => {}
                DisplayItem::Shadow { rect, radius, blur, color } => {
                    self.shadow_clip(t.rect(*rect), radius * t.scale, blur * t.scale, *color, clip)
                }
                DisplayItem::FixedStart => {
                    fixed += 1;
                    t = Transform { dy: t0.dy + self.fixed_shift, ..t0 };
                }
                DisplayItem::FixedEnd => {
                    fixed = fixed.saturating_sub(1);
                    if fixed == 0 {
                        t = t0;
                    }
                }
                DisplayItem::Rect { rect, color } => self.fill_clip(t.rect(*rect), *color, clip),
                DisplayItem::RoundRect { rect, radius, color } => {
                    self.round_rect_clip(t.rect(*rect), radius * t.scale, *color, None, clip)
                }
                DisplayItem::Text {
                    x,
                    baseline,
                    text,
                    font,
                    size,
                    color,
                } => {
                    let b = baseline * t.scale + t.dy;
                    let s = size * t.scale;
                    if b + s < clip.y0 as f32 || b - s * 1.5 > clip.y1 as f32 {
                        continue;
                    }
                    self.text_clip(fonts, x * t.scale + t.dx, b, text, *font, s, *color, clip);
                }
                DisplayItem::Image { rect, image } => self.image_clip(image, t.rect(*rect), clip),
                DisplayItem::RoundBorder {
                    rect,
                    radius,
                    width,
                    color,
                } => self.round_rect_clip(
                    t.rect(*rect),
                    radius * t.scale,
                    *color,
                    Some((width * t.scale).max(1.0)),
                    clip,
                ),
                DisplayItem::Gradient {
                    rect,
                    radius,
                    angle,
                    stops,
                } => self.gradient_clip(t.rect(*rect), radius * t.scale, *angle, stops, clip),
                DisplayItem::BgImage {
                    rect,
                    image,
                    tile,
                    repeat,
                    tint,
                } => {
                    let area = clip.intersect(Clip::from_rect(t.rect(*rect)));
                    self.tiles_clip(image, t.rect(*tile), *repeat, area, *tint);
                }
                DisplayItem::PushClip(r) => clips.push(clip.intersect(Clip::from_rect(t.rect(*r)))),
                DisplayItem::LayerStart { .. } | DisplayItem::LayerEnd | DisplayItem::Hit { .. } => {}
                DisplayItem::Video { rect, node } => {
                    let r = t.rect(*rect);
                    if std::env::var_os("FERRO_MIDIA_LOG").is_some() {
                        eprintln!("[mídia] pinta vídeo {node} em {r:?}: quadro {}", self.videos.contains_key(node));
                    }
                    self.fill_clip(r, Color::BLACK, clip);
                    if let Some(img) = self.videos.get(node).cloned() {
                        // object-fit: contain (barras pretas onde sobrar)
                        let s = (r.w / img.width as f32).min(r.h / img.height as f32);
                        let (w, h) = (img.width as f32 * s, img.height as f32 * s);
                        let fit = Rect { x: r.x + (r.w - w) / 2.0, y: r.y + (r.h - h) / 2.0, w, h };
                        self.image_clip(&img, fit, clip);
                    }
                    if self.paused.contains(node) {
                        // botão de tocar no meio do vídeo
                        let d = (r.w.min(r.h) * 0.22).clamp(36.0 * t.scale, 88.0 * t.scale);
                        let c = Rect { x: r.x + (r.w - d) / 2.0, y: r.y + (r.h - d) / 2.0, w: d, h: d };
                        self.round_rect_clip(c, d / 2.0, Color { r: 0, g: 0, b: 0, a: 150 }, None, clip);
                        let s = d * 0.36;
                        let (cx, cy) = (c.x + d / 2.0 + s * 0.12, c.y + d / 2.0);
                        // triângulo, linha a linha
                        let y0 = (cy - s / 2.0).max(clip.y0 as f32) as i32;
                        let y1 = (cy + s / 2.0).min(clip.y1 as f32) as i32;
                        for py in y0..y1 {
                            let f = 1.0 - ((py as f32 + 0.5 - cy).abs() / (s / 2.0));
                            let w = (s * 0.9 * f).max(0.0);
                            let x0 = (cx - s * 0.45).max(clip.x0 as f32) as i32;
                            let x1 = (cx - s * 0.45 + w).min(clip.x1 as f32) as i32;
                            for px in x0..x1 {
                                self.blend(px, py, Color::WHITE, 255);
                            }
                        }
                    }
                }
                DisplayItem::Canvas { rect, node } => {
                    if let Some(img) = self.videos.get(node).cloned() {
                        self.image_clip(&img, t.rect(*rect), clip);
                    }
                }
                DisplayItem::PopClip => {
                    if clips.len() > 1 {
                        clips.pop();
                    }
                }
            }
        }
    }

