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

/// YUV 4:2:0 (faixa limitada, BT.709) → RGBA.
#[allow(clippy::too_many_arguments)]
pub fn yuv420_to_rgba(w: usize, h: usize, y: &[u8], ys: usize, u: &[u8], v: &[u8], cs: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let yr = &y[row * ys..row * ys + w];
        let ur = &u[(row / 2) * cs..];
        let vr = &v[(row / 2) * cs..];
        let o = &mut out[row * w * 4..(row + 1) * w * 4];
        for col in 0..w {
            let yy = (yr[col] as i32 - 16) * 298;
            let cb = ur[col / 2] as i32 - 128;
            let cr = vr[col / 2] as i32 - 128;
            // coeficientes BT.709 em ponto fixo (×256)
            let r = (yy + 459 * cr + 128) >> 8;
            let g = (yy - 55 * cb - 136 * cr + 128) >> 8;
            let b = (yy + 541 * cb + 128) >> 8;
            let px = &mut o[col * 4..col * 4 + 4];
            px[0] = r.clamp(0, 255) as u8;
            px[1] = g.clamp(0, 255) as u8;
            px[2] = b.clamp(0, 255) as u8;
            px[3] = 255;
        }
    }
    out
}

// ------------------------------------------------------------------ AV1

mod av1 {
    use super::*;
    use rav1d::include::dav1d::data::Dav1dData;
    use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
    use rav1d::include::dav1d::headers::DAV1D_PIXEL_LAYOUT_I420;
    use rav1d::include::dav1d::picture::Dav1dPicture;
    use rav1d::src::lib as d;

    pub struct Av1 {
        ctx: Option<Dav1dContext>,
    }

    // o contexto só é usado pela thread que decodifica
    unsafe impl Send for Av1 {}

    const EAGAIN: i32 = -11;

    impl Av1 {
        pub fn new() -> Option<Av1> {
            unsafe {
                let mut s = std::mem::MaybeUninit::<Dav1dSettings>::zeroed();
                d::dav1d_default_settings(NonNull::new(s.as_mut_ptr())?);
                let mut s = s.assume_init();
                s.n_threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(8)) as i32;
                s.max_frame_delay = 1;
                let mut ctx: Option<Dav1dContext> = None;
                let r = d::dav1d_open(NonNull::new(&mut ctx), NonNull::new(&mut s));
                if r.0 != 0 {
                    return None;
                }
                Some(Av1 { ctx })
            }
        }

        fn pictures(&mut self, out: &mut Vec<Frame>) {
            unsafe {
                loop {
                    let mut pic: Dav1dPicture = Default::default();
                    let r = d::dav1d_get_picture(self.ctx.clone(), NonNull::new(&mut pic));
                    if r.0 != 0 {
                        break;
                    }
                    if let Some(f) = convert(&pic) {
                        out.push(f);
                    }
                    d::dav1d_picture_unref(NonNull::new(&mut pic));
                }
            }
        }

        pub fn decode(&mut self, s: &Sample) -> Result<Vec<Frame>, String> {
            let mut out = Vec::new();
            unsafe {
                let mut data: Dav1dData = Default::default();
                let ptr = d::dav1d_data_create(NonNull::new(&mut data), s.data.len());
                if ptr.is_null() {
                    return Err("av1: sem memória".into());
                }
                std::ptr::copy_nonoverlapping(s.data.as_ptr(), ptr, s.data.len());
                data.m.timestamp = (s.pts * 1_000_000.0).round() as i64;
                loop {
                    let r = d::dav1d_send_data(self.ctx.clone(), NonNull::new(&mut data));
                    if r.0 == EAGAIN {
                        // decodificador cheio: tira quadros e tenta de novo
                        self.pictures(&mut out);
                        continue;
                    }
                    if r.0 != 0 {
                        d::dav1d_data_unref(NonNull::new(&mut data));
                        return Err(format!("av1: erro {}", r.0));
                    }
                    if data.sz == 0 {
                        break;
                    }
                }
            }
            self.pictures(&mut out);
            Ok(out)
        }

        pub fn reset(&mut self) {
            unsafe { d::dav1d_flush(self.ctx.clone().unwrap()) }
        }
    }

    impl Drop for Av1 {
        fn drop(&mut self) {
            unsafe { d::dav1d_close(NonNull::new(&mut self.ctx)) }
        }
    }

    unsafe fn convert(pic: &Dav1dPicture) -> Option<Frame> {
        let (w, h) = (pic.p.w as usize, pic.p.h as usize);
        if pic.p.bpc != 8 || pic.p.layout != DAV1D_PIXEL_LAYOUT_I420 || w == 0 || h == 0 {
            return None;
        }
        let (ys, cs) = (pic.stride[0] as usize, pic.stride[1] as usize);
        let y = unsafe { std::slice::from_raw_parts(pic.data[0]?.as_ptr() as *const u8, ys * h) };
        let ch = h.div_ceil(2);
        let u = unsafe { std::slice::from_raw_parts(pic.data[1]?.as_ptr() as *const u8, cs * ch) };
        let v = unsafe { std::slice::from_raw_parts(pic.data[2]?.as_ptr() as *const u8, cs * ch) };
        Some(Frame::from_planes(pic.m.timestamp as f64 / 1_000_000.0, w, h, y, ys, u, v, cs))
    }
}

// ----------------------------------------------------------------- H.264

pub struct Avc {
    dec: rusty_h264_decoder::Decoder,
    nal_len: usize,
    /// SPS e PPS do avcC, em Annex-B (vão antes de cada quadro-chave).
    header: Vec<u8>,
}

impl Avc {
    fn new(avcc: &[u8]) -> Option<Avc> {
        if avcc.len() < 7 {
            return None;
        }
        let nal_len = (avcc[4] & 3) as usize + 1;
        let mut header = Vec::new();
        let mut i = 5;
        for kind in 0..2 {
            let count = if kind == 0 { (avcc.get(i)? & 31) as usize } else { *avcc.get(i)? as usize };
            i += 1;
            for _ in 0..count {
                let n = u16::from_be_bytes([*avcc.get(i)?, *avcc.get(i + 1)?]) as usize;
                i += 2;
                header.extend_from_slice(&[0, 0, 0, 1]);
                header.extend_from_slice(avcc.get(i..i + n)?);
                i += n;
            }
        }
        Some(Avc {
            dec: rusty_h264_decoder::Decoder::new(),
            nal_len,
            header,
        })
    }

    fn decode(&mut self, s: &Sample) -> Result<Vec<Frame>, String> {
        // tamanho + NAL → código de início + NAL (Annex-B)
        let mut au = Vec::with_capacity(s.data.len() + self.header.len() + 32);
        if s.key {
            au.extend_from_slice(&self.header);
        }
        let d = &s.data;
        let mut i = 0;
        while i + self.nal_len <= d.len() {
            let mut n = 0usize;
            for k in 0..self.nal_len {
                n = (n << 8) | d[i + k] as usize;
            }
            i += self.nal_len;
            if i + n > d.len() {
                break;
            }
            au.extend_from_slice(&[0, 0, 0, 1]);
            au.extend_from_slice(&d[i..i + n]);
            i += n;
        }
        match self.dec.decode(&au) {
            Ok(Some(f)) => {
                let (w, h) = (f.width, f.height);
                let frame = Frame::from_planes(s.pts, w, h, &f.y, w, &f.u, &f.v, w / 2);
                self.dec.recycle_frame(f);
                Ok(vec![frame])
            }
            Ok(None) => Ok(Vec::new()),
            Err(e) => Err(format!("h264: {e:?}")),
        }
    }
}

/// Decodificador de vídeo de uma trilha.
pub enum VideoDecoder {
    Av1(av1::Av1),
    Avc(Box<Avc>),
}

impl VideoDecoder {
    pub fn new(codec: &Codec) -> Option<VideoDecoder> {
        match codec {
            Codec::Av1 { .. } => av1::Av1::new().map(VideoDecoder::Av1),
            Codec::Avc { config } => Avc::new(config).map(|a| VideoDecoder::Avc(Box::new(a))),
            _ => None,
        }
    }

    pub fn decode(&mut self, s: &Sample) -> Result<Vec<Frame>, String> {
        match self {
            VideoDecoder::Av1(d) => d.decode(s),
            VideoDecoder::Avc(d) => d.decode(s),
        }
    }

    /// Antes de pular para outro ponto (busca): esquece o que estava no meio.
    pub fn reset(&mut self) {
        match self {
            VideoDecoder::Av1(d) => d.reset(),
            VideoDecoder::Avc(d) => d.dec = rusty_h264_decoder::Decoder::new(),
        }
    }
}

// ------------------------------------------------------------------- AAC

pub struct AudioDecoder {
    dec: symphonia_codec_aac::AacDecoder,
    pub sample_rate: u32,
    pub channels: usize,
}

impl AudioDecoder {
    pub fn new(codec: &Codec) -> Option<AudioDecoder> {
        use symphonia_core::codecs::audio::well_known::CODEC_ID_AAC;
        use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
        let Codec::Aac { config, channels, sample_rate } = codec else { return None };
        let mut params = AudioCodecParameters::new();
        params.for_codec(CODEC_ID_AAC).with_sample_rate(*sample_rate);
        if !config.is_empty() {
            params.with_extra_data(config.clone().into_boxed_slice());
        }
        let dec = symphonia_codec_aac::AacDecoder::try_new(&params, &AudioDecoderOptions::default()).ok()?;
        Some(AudioDecoder { dec, sample_rate: *sample_rate, channels: (*channels).max(1) as usize })
    }

    /// Amostras PCM intercaladas (f32), já no formato da trilha.
    pub fn decode(&mut self, s: &Sample) -> Result<Vec<f32>, String> {
        use symphonia_core::codecs::audio::AudioDecoder as _;
        use symphonia_core::packet::PacketRef;
        use symphonia_core::units::{Duration, Timestamp};
        let packet = PacketRef::new(0, Timestamp::new(0), Duration::new(0), &s.data);
        let buf = self.dec.decode_ref(&packet).map_err(|e| format!("aac: {e}"))?;
        let spec = buf.spec().clone();
        self.sample_rate = spec.rate();
        self.channels = spec.channels().count().max(1);
        let mut out: Vec<f32> = Vec::new();
        buf.copy_to_vec_interleaved(&mut out);
        Ok(out)
    }

    pub fn reset(&mut self) {
        use symphonia_core::codecs::audio::AudioDecoder as _;
        self.dec.reset();
    }
}
