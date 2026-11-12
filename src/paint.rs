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

    /// Sombra desfocada: camadas de retângulos arredondados que vão
    /// crescendo, cada uma um pouco transparente (aproxima a gaussiana).
    fn shadow_clip(&mut self, r: Rect, radius: f32, blur: f32, color: Color, clip: Clip) {
        if color.a == 0 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        if blur < 1.0 {
            self.round_rect_clip(r, radius, color, None, clip);
            return;
        }
        let steps = ((blur / 2.0).ceil() as usize).clamp(3, 10);
        // alfa de cada camada para que o miolo chegue perto do alfa pedido
        let total = color.a as f32 / 255.0;
        let each = 1.0 - (1.0 - total).powf(1.0 / steps as f32);
        let c = Color { a: (each * 255.0).round().max(1.0) as u8, ..color };
        for i in 0..steps {
            // de fora (blur/2 maior) para dentro (blur/2 menor)
            let grow = blur / 2.0 - blur * i as f32 / (steps - 1).max(1) as f32;
            let rr = Rect { x: r.x - grow, y: r.y - grow, w: r.w + 2.0 * grow, h: r.h + 2.0 * grow };
            if rr.w <= 0.0 || rr.h <= 0.0 {
                continue;
            }
            self.round_rect_clip(rr, (radius + grow).max(0.0), c, None, clip);
        }
    }

    /// Desenha `items` transformados por `m` em torno de `origin`: pinta o
    /// trecho à parte (sobre preto e sobre branco, para recuperar o alfa) e
    /// aplica a matriz com amostragem bilinear.
    fn paint_transformed(&mut self, items: &[DisplayItem], fonts: &Fonts, t: Transform, m: [f32; 6], origin: (f32, f32), clip: Clip) {
        // limites do trecho (documento)
        let mut b: Option<Rect> = None;
        let mut add = |r: Rect| {
            if r.w <= 0.0 || r.h <= 0.0 {
                return;
            }
            b = Some(match b {
                None => r,
                Some(o) => {
                    let x0 = o.x.min(r.x);
                    let y0 = o.y.min(r.y);
                    Rect { x: x0, y: y0, w: (o.x + o.w).max(r.x + r.w) - x0, h: (o.y + o.h).max(r.y + r.h) - y0 }
                }
            });
        };
        for it in items {
            match it {
                DisplayItem::Rect { rect, .. }
                | DisplayItem::RoundRect { rect, .. }
                | DisplayItem::Image { rect, .. }
                | DisplayItem::RoundBorder { rect, .. }
                | DisplayItem::Gradient { rect, .. }
                | DisplayItem::BgImage { rect, .. }
                | DisplayItem::Video { rect, .. }
                | DisplayItem::Canvas { rect, .. } => add(*rect),
                DisplayItem::Shadow { rect, blur, .. } => {
                    add(Rect { x: rect.x - blur, y: rect.y - blur, w: rect.w + 2.0 * blur, h: rect.h + 2.0 * blur })
                }
                DisplayItem::Text { x, baseline, text, font, size, .. } => {
                    let w = fonts.measure(*font, *size, text);
                    add(Rect { x: *x, y: baseline - size * 1.1, w: w + 2.0, h: size * 1.5 });
                }
                _ => {}
            }
        }
        let Some(bounds) = b else { return };
        let s = t.scale;
        let (w, h) = ((bounds.w * s).ceil() as usize + 2, (bounds.h * s).ceil() as usize + 2);
        let det = m[0] * m[3] - m[1] * m[2];
        if w * h > 4096 * 4096 || det.abs() < 1e-6 {
            // grande demais ou achatado: pinta sem transformar (ou some, se achatado)
            if det.abs() >= 1e-6 {
                self.paint_list(items, fonts, t, None);
            }
            return;
        }
        let off = Transform { scale: s, dx: -bounds.x * s + 1.0, dy: -bounds.y * s + 1.0 };
        let render = |bg: Color| {
            let mut c = Canvas::new(w, h, bg);
            c.videos = self.videos.clone();
            c.paint_list(items, fonts, off, None);
            c.pixels
        };
        let black = render(Color::BLACK);
        let white = render(Color::WHITE);
        // documento: p' = origin + M·(p - origin) + (e, f); inversa para amostrar
        let (ox, oy) = origin;
        let inv = [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det];
        let fwd = |x: f32, y: f32| {
            let (dx, dy) = (x - ox, y - oy);
            (ox + m[0] * dx + m[2] * dy + m[4], oy + m[1] * dx + m[3] * dy + m[5])
        };
        // caixa de destino (dispositivo)
        let corners = [
            fwd(bounds.x, bounds.y),
            fwd(bounds.x + bounds.w, bounds.y),
            fwd(bounds.x, bounds.y + bounds.h),
            fwd(bounds.x + bounds.w, bounds.y + bounds.h),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (cx, cy) in corners {
            let (dx, dy) = (cx * s + t.dx, cy * s + t.dy);
            x0 = x0.min(dx);
            y0 = y0.min(dy);
            x1 = x1.max(dx);
            y1 = y1.max(dy);
        }
        let area = clip
            .intersect(Clip { x0: 0, y0: 0, x1: self.width as i32, y1: self.height as i32 })
            .intersect(Clip { x0: x0.floor() as i32, y0: y0.floor() as i32, x1: x1.ceil() as i32 + 1, y1: y1.ceil() as i32 + 1 });
        if area.is_empty() {
            return;
        }
        let sample = |buf: &[u32], fx: f32, fy: f32| -> Option<[f32; 3]> {
            if fx < -1.0 || fy < -1.0 || fx > w as f32 || fy > h as f32 {
                return None;
            }
            let (ix, iy) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - ix, fy - iy);
            let at = |x: f32, y: f32| -> [f32; 3] {
                let (x, y) = ((x as isize).clamp(0, w as isize - 1) as usize, (y as isize).clamp(0, h as isize - 1) as usize);
                let p = buf[y * w + x];
                [((p >> 16) & 255) as f32, ((p >> 8) & 255) as f32, (p & 255) as f32]
            };
            let (a, b2, c, d) = (at(ix, iy), at(ix + 1.0, iy), at(ix, iy + 1.0), at(ix + 1.0, iy + 1.0));
            let mut out = [0.0f32; 3];
            for k in 0..3 {
                let top = a[k] + (b2[k] - a[k]) * tx;
                let bot = c[k] + (d[k] - c[k]) * tx;
                out[k] = top + (bot - top) * ty;
            }
            Some(out)
        };
        for py in area.y0..area.y1 {
            for px in area.x0..area.x1 {
                // dispositivo → documento → antes da transformação → fora da tela
                let (dx, dy) = ((px as f32 + 0.5 - t.dx) / s - ox - m[4], (py as f32 + 0.5 - t.dy) / s - oy - m[5]);
                let (ux, uy) = (ox + inv[0] * dx + inv[2] * dy, oy + inv[1] * dx + inv[3] * dy);
                let (fx, fy) = ((ux - bounds.x) * s + 1.0 - 0.5, (uy - bounds.y) * s + 1.0 - 0.5);
                let (Some(bk), Some(wh)) = (sample(&black, fx, fy), sample(&white, fx, fy)) else { continue };
                // alfa = 1 - (branco - preto); cor (pré-multiplicada) = preto
                let alpha = 1.0 - ((wh[0] - bk[0]) + (wh[1] - bk[1]) + (wh[2] - bk[2])) / (3.0 * 255.0);
                if alpha <= 0.002 {
                    continue;
                }
                let alpha = alpha.min(1.0);
                let i = py as usize * self.width + px as usize;
                let dst = self.pixels[i];
                let (dr, dg, db) = (((dst >> 16) & 255) as f32, ((dst >> 8) & 255) as f32, (dst & 255) as f32);
                let r = (bk[0] + dr * (1.0 - alpha)).round().clamp(0.0, 255.0) as u32;
                let g = (bk[1] + dg * (1.0 - alpha)).round().clamp(0.0, 255.0) as u32;
                let bl = (bk[2] + db * (1.0 - alpha)).round().clamp(0.0, 255.0) as u32;
                self.pixels[i] = r << 16 | g << 8 | bl;
            }
        }
    }

    // ---------------------------------------------------- primitivas públicas
    // (coordenadas em pixels do dispositivo)

    pub fn fill_rect(&mut self, r: Rect, color: Color, clip: Option<Rect>) {
        let c = self.clip(clip);
        self.fill_clip(r, color, c);
    }

    pub fn fill_round_rect(&mut self, r: Rect, radius: f32, color: Color, clip: Option<Rect>) {
        let c = self.clip(clip);
        self.round_rect_clip(r, radius, color, None, c);
    }

    /// Contorno de `width` pixels, por dentro do retângulo.
    pub fn stroke_round_rect(&mut self, r: Rect, radius: f32, width: f32, color: Color, clip: Option<Rect>) {
        let c = self.clip(clip);
        self.round_rect_clip(r, radius, color, Some(width), c);
    }

    /// Retângulo arredondado com gradiente vertical de `top` para `bottom`.
    pub fn fill_round_rect_gradient(&mut self, r: Rect, radius: f32, top: Color, bottom: Color) {
        let c = self.full().intersect(Clip::from_rect(r));
        if c.is_empty() {
            return;
        }
        let radius = radius.clamp(0.0, r.w.min(r.h) / 2.0);
        for y in c.y0..c.y1 {
            let py = y as f32 + 0.5;
            let color = lerp_color(top, bottom, ((py - r.y) / r.h).clamp(0.0, 1.0));
            for x in c.x0..c.x1 {
                let cov = (0.5 - rr_sdf(x as f32 + 0.5, py, r, radius)).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, color, (cov * 255.0) as u32);
                }
            }
        }
    }

    /// Pinta `color` por fora dos cantos arredondados de `r` (arredonda o que
    /// já foi desenhado dentro do retângulo).
    pub fn round_corners(&mut self, r: Rect, radius: f32, color: Color) {
        let radius = radius.clamp(0.0, r.w.min(r.h) / 2.0);
        let full = self.full();
        let corners = [
            (r.x, r.y),
            (r.x + r.w - radius, r.y),
            (r.x, r.y + r.h - radius),
            (r.x + r.w - radius, r.y + r.h - radius),
        ];
        for (cx, cy) in corners {
            let c = full.intersect(Clip::from_rect(Rect {
                x: cx,
                y: cy,
                w: radius,
                h: radius,
            }));
            for y in c.y0..c.y1 {
                for x in c.x0..c.x1 {
                    let inside = (0.5 - rr_sdf(x as f32 + 0.5, y as f32 + 0.5, r, radius)).clamp(0.0, 1.0);
                    if inside < 1.0 {
                        self.blend(x, y, color, ((1.0 - inside) * 255.0) as u32);
                    }
                }
            }
        }
    }

    /// Vidro fosco: desfoca o que já está pintado dentro do retângulo
    /// arredondado `r`. Reduz a região 4x, aplica três passadas de média
    /// móvel (aproxima um gaussiano) e amplia de volta com interpolação.
    pub fn frost(&mut self, r: Rect, radius: f32, blur: f32) {
        const F: usize = 4;
        let full = self.full();
        let pad = (blur * 2.0).ceil() as i32;
        let c = full.intersect(Clip::from_rect(r));
        if c.is_empty() {
            return;
        }
        let e = full.intersect(Clip {
            x0: c.x0 - pad,
            y0: c.y0 - pad,
            x1: c.x1 + pad,
            y1: c.y1 + pad,
        });
        let (ew, eh) = ((e.x1 - e.x0) as usize, (e.y1 - e.y0) as usize);
        let (sw, sh) = (ew.div_ceil(F), eh.div_ceil(F));
        // reduz por média de blocos F x F
        let mut small = vec![[0.0f32; 3]; sw * sh];
        let mut count = vec![0.0f32; sw * sh];
        for y in 0..eh {
            let row = (e.y0 as usize + y) * self.width + e.x0 as usize;
            for x in 0..ew {
                let p = self.pixels[row + x];
                let i = (y / F) * sw + x / F;
                small[i][0] += ((p >> 16) & 0xff) as f32;
                small[i][1] += ((p >> 8) & 0xff) as f32;
                small[i][2] += (p & 0xff) as f32;
                count[i] += 1.0;
            }
        }
        for (v, n) in small.iter_mut().zip(&count) {
            for ch in v.iter_mut() {
                *ch /= n.max(1.0);
            }
        }
        // média móvel horizontal e vertical, 3 vezes
        let rad = ((blur / F as f32).round() as usize).max(1);
        let mut tmp = small.clone();
        for _ in 0..3 {
            for (horizontal, len, lines) in [(true, sw, sh), (false, sh, sw)] {
                for line in 0..lines {
                    let idx = |k: usize| if horizontal { line * sw + k } else { k * sw + line };
                    let mut acc = [0.0f32; 3];
                    let win = (2 * rad + 1) as f32;
                    for k in 0..=2 * rad {
                        let src = small[idx(k.saturating_sub(rad).min(len - 1))];
                        for ch in 0..3 {
                            acc[ch] += src[ch];
                        }
                    }
                    for k in 0..len {
                        for ch in 0..3 {
                            tmp[idx(k)][ch] = acc[ch] / win;
                        }
                        let out = small[idx(k.saturating_sub(rad))];
                        let inn = small[idx((k + rad + 1).min(len - 1))];
                        for ch in 0..3 {
                            acc[ch] += inn[ch] - out[ch];
                        }
                    }
                }
                std::mem::swap(&mut small, &mut tmp);
            }
        }
        // amplia e escreve dentro da forma arredondada
        let radius = radius.clamp(0.0, r.w.min(r.h) / 2.0);
        for y in c.y0..c.y1 {
            let fy = (((y - e.y0) as f32 + 0.5) / F as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(sh - 1);
            for x in c.x0..c.x1 {
                let cov = (0.5 - rr_sdf(x as f32 + 0.5, y as f32 + 0.5, r, radius)).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let fx = (((x - e.x0) as f32 + 0.5) / F as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(sw - 1);
                let s = |xx: usize, yy: usize| small[yy * sw + xx];
                let (a, b, cc, d) = (s(x0, y0), s(x1, y0), s(x0, y1), s(x1, y1));
                let mut col = [0u8; 3];
                for ch in 0..3 {
                    let top = a[ch] + (b[ch] - a[ch]) * tx;
                    let bot = cc[ch] + (d[ch] - cc[ch]) * tx;
                    col[ch] = (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8;
                }
                self.blend(x, y, Color::rgb(col[0], col[1], col[2]), (cov * 255.0) as u32);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &mut self,
        fonts: &Fonts,
        x: f32,
        baseline: f32,
        text: &str,
        font: FontId,
        size: f32,
        color: Color,
        clip: Option<Rect>,
    ) {
        let c = self.clip(clip);
        self.text_clip(fonts, x, baseline, text, font, size, color, c);
    }

    pub fn draw_image(&mut self, img: &ImageData, r: Rect, clip: Option<Rect>) {
        let c = self.clip(clip);
        self.image_clip(img, r, c);
    }

    // ---------------------------------------------------------- implementação

    #[inline]
    fn blend(&mut self, x: i32, y: i32, color: Color, coverage: u32) {
        let a = color.a as u32 * coverage / 255;
        if a == 0 {
            return;
        }
        let i = y as usize * self.width + x as usize;
        self.pixels[i] = if a == 255 {
            pack(color)
        } else {
            mix(self.pixels[i], color.r as u32, color.g as u32, color.b as u32, a)
        };
    }

    fn fill_clip(&mut self, r: Rect, color: Color, clip: Clip) {
        if color.a == 0 {
            return;
        }
        let c = clip.intersect(Clip {
            x0: r.x.round() as i32,
            y0: r.y.round() as i32,
            x1: (r.x + r.w).round() as i32,
            y1: (r.y + r.h).round() as i32,
        });
        if c.is_empty() {
            return;
        }
        if color.a == 255 {
            let p = pack(color);
            for y in c.y0..c.y1 {
                let row = y as usize * self.width;
                self.pixels[row + c.x0 as usize..row + c.x1 as usize].fill(p);
            }
        } else {
            for y in c.y0..c.y1 {
                for x in c.x0..c.x1 {
                    self.blend(x, y, color, 255);
                }
            }
        }
    }

    /// Retângulo arredondado com antialias via função de distância.
    /// `stroke = Some(w)` desenha só um contorno de largura `w`.
    fn round_rect_clip(&mut self, r: Rect, radius: f32, color: Color, stroke: Option<f32>, clip: Clip) {
        if color.a == 0 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let radius = radius.clamp(0.0, r.w.min(r.h) / 2.0);
        let c = clip.intersect(Clip::from_rect(r));
        if c.is_empty() {
            return;
        }
        let sdf = rr_sdf;
        let inner = stroke.map(|w| {
            (
                Rect {
                    x: r.x + w,
                    y: r.y + w,
                    w: (r.w - 2.0 * w).max(0.0),
                    h: (r.h - 2.0 * w).max(0.0),
                },
                (radius - w).max(0.0),
            )
        });
        for y in c.y0..c.y1 {
            let py = y as f32 + 0.5;
            for x in c.x0..c.x1 {
                let px = x as f32 + 0.5;
                let mut cov = (0.5 - sdf(px, py, r, radius)).clamp(0.0, 1.0);
                if let Some((ir, irad)) = inner
                    && cov > 0.0
                {
                    let inside = if ir.w > 0.0 && ir.h > 0.0 {
                        (0.5 - sdf(px, py, ir, irad)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    cov = (cov - inside).max(0.0);
                }
                if cov > 0.0 {
                    self.blend(x, y, color, (cov * 255.0) as u32);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn text_clip(
        &mut self,
        fonts: &Fonts,
        x: f32,
        baseline: f32,
        text: &str,
        font: FontId,
        size: f32,
        color: Color,
        clip: Clip,
    ) {
        if color.a == 0 || clip.is_empty() {
            return;
        }
        let mut pen = x;
        let base = baseline.round() as i32;
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if pen > clip.x1 as f32 {
                break;
            }
            let emoji = chars.peek() == Some(&'\u{FE0F}');
            if let Some(cg) = fonts.color_glyph(font, ch, size, emoji) {
                // emoji colorido: pinta as cores dele (a transparência do texto vale)
                let m = cg.metrics;
                let gx = pen.round() as i32 + m.xmin;
                let gy = base - m.height as i32 - m.ymin;
                for row in 0..m.height {
                    let py = gy + row as i32;
                    if py < clip.y0 || py >= clip.y1 {
                        continue;
                    }
                    for col in 0..m.width {
                        let px = gx + col as i32;
                        if px < clip.x0 || px >= clip.x1 {
                            continue;
                        }
                        let p = &cg.rgba[(row * m.width + col) * 4..(row * m.width + col) * 4 + 4];
                        let a = p[3] as u32 * color.a as u32 / 255;
                        if a > 0 {
                            self.blend(px, py, Color::rgb(p[0], p[1], p[2]), a);
                        }
                    }
                }
            } else if !ch.is_whitespace()
                && let Some(g) = fonts.glyph(font, ch, size)
            {
                let m = g.metrics;
                let gx = pen.round() as i32 + m.xmin;
                let gy = base - m.height as i32 - m.ymin;
                for row in 0..m.height {
                    let py = gy + row as i32;
                    if py < clip.y0 || py >= clip.y1 {
                        continue;
                    }
                    for col in 0..m.width {
                        let px = gx + col as i32;
                        if px < clip.x0 || px >= clip.x1 {
                            continue;
                        }
                        let cov = g.coverage[row * m.width + col];
                        if cov > 0 {
                            self.blend(px, py, color, cov as u32);
                        }
                    }
                }
            }
            pen += fonts.advance(font, ch, size);
        }
    }

    fn gradient_clip(&mut self, r: Rect, radius: f32, angle: f32, stops: &[(Color, f32)], clip: Clip) {
        let c = clip.intersect(Clip::from_rect(r));
        if c.is_empty() || stops.is_empty() {
            return;
        }
        // tabela de 256 cores ao longo da linha do gradiente
        let lut: Vec<Color> = (0..256)
            .map(|i| {
                let t = i as f32 / 255.0;
                match stops.iter().position(|s| s.1 >= t) {
                    Some(0) => stops[0].0,
                    None => stops[stops.len() - 1].0,
                    Some(k) => {
                        let (a, b) = (stops[k - 1], stops[k]);
                        let span = (b.1 - a.1).max(1e-6);
                        lerp_color(a.0, b.0, ((t - a.1) / span).clamp(0.0, 1.0))
                    }
                }
            })
            .collect();
        let a = angle.to_radians();
        let (dx, dy) = (a.sin(), -a.cos());
        let len = (r.w * dx.abs() + r.h * dy.abs()).max(1.0);
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let radius = radius.clamp(0.0, r.w.min(r.h) / 2.0);
        for y in c.y0..c.y1 {
            let py = y as f32 + 0.5;
            for x in c.x0..c.x1 {
                let px = x as f32 + 0.5;
                let cov = if radius > 0.0 {
                    (0.5 - rr_sdf(px, py, r, radius)).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                if cov <= 0.0 {
                    continue;
                }
                let t = (((px - cx) * dx + (py - cy) * dy) / len + 0.5).clamp(0.0, 1.0);
                let col = lut[(t * 255.0) as usize];
                self.blend(x, y, col, (cov * 255.0) as u32);
            }
        }
    }

    /// Repete uma imagem a partir de `tile` para cobrir a área.
    fn tiles_clip(&mut self, img: &ImageData, tile: Rect, repeat: (bool, bool), area: Clip, tint: Option<Color>) {
        if area.is_empty() || tile.w < 0.5 || tile.h < 0.5 {
            return;
        }
        let start = |pos: f32, size: f32, lo: i32, rep: bool| {
            if rep { pos - ((pos - lo as f32) / size).ceil() * size } else { pos }
        };
        let x0 = start(tile.x, tile.w, area.x0, repeat.0);
        let y0 = start(tile.y, tile.h, area.y0, repeat.1);
        let mut count = 0;
        let mut y = y0;
        while y < area.y1 as f32 && count < 4000 {
            let mut x = x0;
            while x < area.x1 as f32 && count < 4000 {
                self.image_tinted(img, Rect { x, y, w: tile.w, h: tile.h }, area, tint);
                count += 1;
                if !repeat.0 {
                    break;
                }
                x += tile.w;
            }
            if !repeat.1 {
                break;
            }
            y += tile.h;
        }
    }

    fn image_clip(&mut self, img: &ImageData, r: Rect, clip: Clip) {
        self.image_tinted(img, r, clip, None);
    }

    /// Imagem escalada: bilinear para ampliar, média de amostras para reduzir.
    /// Com `tint`, usa só o alfa da imagem e pinta com essa cor.
    fn image_tinted(&mut self, img: &ImageData, r: Rect, clip: Clip, tint: Option<Color>) {
        if img.width == 0 || img.height == 0 || r.w < 0.5 || r.h < 0.5 {
            return;
        }
        let c = clip.intersect(Clip {
            x0: r.x.round() as i32,
            y0: r.y.round() as i32,
            x1: (r.x + r.w).round() as i32,
            y1: (r.y + r.h).round() as i32,
        });
        if c.is_empty() {
            return;
        }
        let (iw, ih) = (img.width as usize, img.height as usize);
        let sx = iw as f32 / r.w;
        let sy = ih as f32 / r.h;
        let px = |x: usize, y: usize| {
            let i = (y * iw + x) * 4;
            [img.rgba[i] as f32, img.rgba[i + 1] as f32, img.rgba[i + 2] as f32, img.rgba[i + 3] as f32]
        };
        // número de amostras por eixo ao reduzir
        let nx = sx.ceil().clamp(1.0, 4.0) as usize;
        let ny = sy.ceil().clamp(1.0, 4.0) as usize;
        for y in c.y0..c.y1 {
            for x in c.x0..c.x1 {
                let mut acc = [0.0f32; 4];
                for j in 0..ny {
                    for i in 0..nx {
                        let fx = (x as f32 + (i as f32 + 0.5) / nx as f32 - r.x) * sx - 0.5;
                        let fy = (y as f32 + (j as f32 + 0.5) / ny as f32 - r.y) * sy - 0.5;
                        let fx = fx.clamp(0.0, (iw - 1) as f32);
                        let fy = fy.clamp(0.0, (ih - 1) as f32);
                        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                        let (x1, y1) = ((x0 + 1).min(iw - 1), (y0 + 1).min(ih - 1));
                        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                        let (a, b, cc, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
                        for k in 0..4 {
                            let top = a[k] + (b[k] - a[k]) * tx;
                            let bot = cc[k] + (d[k] - cc[k]) * tx;
                            acc[k] += top + (bot - top) * ty;
                        }
                    }
                }
                let n = (nx * ny) as f32;
                let col = match tint {
                    Some(t) => Color {
                        a: ((acc[3] / n) * t.a as f32 / 255.0) as u8,
                        ..t
                    },
                    None => Color {
                        r: (acc[0] / n) as u8,
                        g: (acc[1] / n) as u8,
                        b: (acc[2] / n) as u8,
                        a: (acc[3] / n) as u8,
                    },
                };
                self.blend(x, y, col, 255);
            }
        }
    }

    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width as u32, self.height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        let mut data = Vec::with_capacity(self.pixels.len() * 3);
        for p in &self.pixels {
            data.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        w.write_image_data(&data).map_err(|e| e.to_string())
    }
}
