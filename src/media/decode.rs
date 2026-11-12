//! Decodificadores (todos em Rust): AV1 (rav1d), H.264 Baseline/Main
//! (rusty_h264) e AAC (symphonia). Os quadros saem em RGBA, prontos para a tela.

use std::ptr::NonNull;
use std::sync::Arc;

use super::mp4::{Codec, Sample};
use crate::paint::ImageData;

/// Um quadro de vídeo decodificado, em YUV 4:2:0 compacto (o RGBA só é
/// feito na hora de mostrar: guarda 2,7× menos memória na fila).
pub struct Frame {
    pub pts: f64,
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Frame {
    fn from_planes(pts: f64, w: usize, h: usize, y: &[u8], ys: usize, u: &[u8], v: &[u8], cs: usize) -> Frame {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut fy = Vec::with_capacity(w * h);
        for r in 0..h {
            fy.extend_from_slice(&y[r * ys..r * ys + w]);
        }
        let mut fu = Vec::with_capacity(cw * ch);
        let mut fv = Vec::with_capacity(cw * ch);
        for r in 0..ch {
            fu.extend_from_slice(&u[r * cs..r * cs + cw]);
            fv.extend_from_slice(&v[r * cs..r * cs + cw]);
        }
        Frame { pts, width: w, height: h, y: fy, u: fu, v: fv }
    }

    /// RGBA para a tela.
    pub fn to_image(&self) -> Arc<ImageData> {
        let rgba = yuv420_to_rgba(self.width, self.height, &self.y, self.width, &self.u, &self.v, self.width.div_ceil(2));
        Arc::new(ImageData { width: self.width as u32, height: self.height as u32, rgba })
    }
}

