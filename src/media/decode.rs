//! Decodificadores (todos em Rust): AV1 (rav1d), H.264 Baseline/Main
//! (rusty_h264), VP9 (rusty_vp9), AAC (symphonia) e Opus (opus1). Os quadros
//! saem em YUV 4:2:0 e viram RGBA na hora de mostrar.

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

// ------------------------------------------------------------------ VP9

pub struct Vp9 {
    dec: rusty_vp9::Vp9Decoder,
}

impl Vp9 {
    fn decode(&mut self, s: &Sample) -> Result<Vec<Frame>, String> {
        let pts = (s.pts * 1_000_000.0).round() as i64;
        self.dec.push(&s.data, Some(pts)).map_err(|e| format!("vp9: {e:?}"))?;
        let mut out = Vec::new();
        loop {
            match self.dec.next_frame() {
                Ok(f) => {
                    if let Some(fr) = vp9_frame(&f) {
                        out.push(fr);
                    }
                }
                Err(rusty_vp9::Error::Again | rusty_vp9::Error::Eof) => break,
                Err(e) => return Err(format!("vp9: {e:?}")),
            }
        }
        Ok(out)
    }
}

/// Quadro VP9 → YUV 4:2:0 de 8 bits (10/12 bits são reduzidos).
fn vp9_frame(f: &rusty_vp9::DecodedFrame) -> Option<Frame> {
    if f.subsampling_x != 1 || f.subsampling_y != 1 || f.planes.len() < 3 {
        return None;
    }
    let (w, h) = (f.width as usize, f.height as usize);
    let pts = f.pts.unwrap_or(0) as f64 / 1_000_000.0;
    if f.bit_depth == 8 {
        return Some(Frame::from_planes(pts, w, h, &f.planes[0], f.strides[0], &f.planes[1], &f.planes[2], f.strides[1]));
    }
    // amostras de 16 bits (little-endian): fica com os 8 mais altos
    let shift = f.bit_depth - 8;
    let to8 = |p: &[u8], stride: usize, pw: usize, ph: usize| {
        let mut out = Vec::with_capacity(pw * ph);
        for r in 0..ph {
            let row = &p[r * stride..];
            for c in 0..pw {
                let v = u16::from_le_bytes([row[c * 2], row[c * 2 + 1]]);
                out.push((v >> shift).min(255) as u8);
            }
        }
        out
    };
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y = to8(&f.planes[0], f.strides[0], w, h);
    let u = to8(&f.planes[1], f.strides[1], cw, ch);
    let v = to8(&f.planes[2], f.strides[2], cw, ch);
    Some(Frame { pts, width: w, height: h, y, u, v })
}

// ------------------------------------------------------------------ VP8

pub struct Vp8 {
    dec: vp8_decoder::Decoder,
}

impl Vp8 {
    fn decode(&mut self, s: &Sample) -> Result<Vec<Frame>, String> {
        match self.dec.decode(&s.data) {
            Ok(Some(p)) => {
                let (w, h) = (p.width as usize, p.height as usize);
                Ok(vec![Frame::from_planes(s.pts, w, h, p.y, p.y_stride, p.u, p.v, p.uv_stride)])
            }
            Ok(None) => Ok(Vec::new()),
            // depois de uma busca, espera o próximo quadro-chave
            Err(vp8_decoder::DecodeError::NeedKeyframe) => Ok(Vec::new()),
            Err(e) => Err(format!("vp8: {e:?}")),
        }
    }
}

/// Decodificador de vídeo de uma trilha.
pub enum VideoDecoder {
    Av1(av1::Av1),
    Avc(Box<Avc>),
    Vp9(Box<Vp9>),
    Vp8(Box<Vp8>),
}

impl VideoDecoder {
    pub fn new(codec: &Codec) -> Option<VideoDecoder> {
        match codec {
            Codec::Av1 { .. } => av1::Av1::new().map(VideoDecoder::Av1),
            Codec::Avc { config } => Avc::new(config).map(|a| VideoDecoder::Avc(Box::new(a))),
            Codec::Vp9 => Some(VideoDecoder::Vp9(Box::new(Vp9 { dec: rusty_vp9::Vp9Decoder::new() }))),
            Codec::Vp8 => Some(VideoDecoder::Vp8(Box::new(Vp8 { dec: vp8_decoder::Decoder::new() }))),
            _ => None,
        }
    }

    pub fn decode(&mut self, s: &Sample) -> Result<Vec<Frame>, String> {
        match self {
            VideoDecoder::Av1(d) => d.decode(s),
            VideoDecoder::Avc(d) => d.decode(s),
            VideoDecoder::Vp9(d) => d.decode(s),
            VideoDecoder::Vp8(d) => d.decode(s),
        }
    }

    /// Antes de pular para outro ponto (busca): esquece o que estava no meio.
    pub fn reset(&mut self) {
        match self {
            VideoDecoder::Av1(d) => d.reset(),
            VideoDecoder::Avc(d) => d.dec = rusty_h264_decoder::Decoder::new(),
            VideoDecoder::Vp9(d) => d.dec = rusty_vp9::Vp9Decoder::new(),
            VideoDecoder::Vp8(d) => d.dec = vp8_decoder::Decoder::new(),
        }
    }
}

// ------------------------------------------------------------------- AAC

enum AudioInner {
    Aac(Box<symphonia_codec_aac::AacDecoder>),
    Vorbis(Box<symphonia_codec_vorbis::VorbisDecoder>),
    Opus { dec: Box<opus1::multistream::MultistreamDecoder>, channels: usize, buf: Vec<f32> },
}

pub struct AudioDecoder {
    dec: AudioInner,
    pub sample_rate: u32,
    pub channels: usize,
}

/// Decodificador Opus a partir do OpusHead (mapa 0: mono/estéreo; 1: até 8
/// canais). Mais de 2 canais vira estéreo na saída.
fn opus_decoder(head: &[u8]) -> Option<AudioDecoder> {
    use opus1::SampleRate;
    use opus1::multistream::MultistreamDecoder;
    let channels = (*head.get(9).unwrap_or(&2)).max(1) as usize;
    let family = *head.get(18).unwrap_or(&0);
    let dec = if family == 0 || head.len() < 21 + channels {
        let c = channels.min(2);
        let mapping: &[u8] = if c == 2 { &[0, 1] } else { &[0] };
        MultistreamDecoder::new(SampleRate::Hz48000, c, 1, (c == 2) as usize, mapping).ok()?
    } else {
        let (streams, coupled) = (head[19] as usize, head[20] as usize);
        MultistreamDecoder::new(SampleRate::Hz48000, channels, streams, coupled, &head[21..21 + channels]).ok()?
    };
    let channels = if family == 0 { channels.min(2) } else { channels };
    Some(AudioDecoder {
        dec: AudioInner::Opus { dec: Box::new(dec), channels, buf: vec![0.0; 5760 * channels] },
        sample_rate: 48000,
        channels: channels.min(2),
    })
}

impl AudioDecoder {
    pub fn new(codec: &Codec) -> Option<AudioDecoder> {
        use symphonia_core::codecs::audio::well_known::CODEC_ID_AAC;
        use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
        if let Codec::Opus { head } = codec {
            return opus_decoder(head);
        }
        if let Codec::Vorbis { headers } = codec {
            use symphonia_core::codecs::audio::well_known::CODEC_ID_VORBIS;
            let mut params = AudioCodecParameters::new();
            params.for_codec(CODEC_ID_VORBIS).with_extra_data(headers.clone().into_boxed_slice());
            let dec = symphonia_codec_vorbis::VorbisDecoder::try_new(&params, &AudioDecoderOptions::default()).ok()?;
            return Some(AudioDecoder { dec: AudioInner::Vorbis(Box::new(dec)), sample_rate: 48000, channels: 2 });
        }
        let Codec::Aac { config, channels, sample_rate } = codec else { return None };
        let mut params = AudioCodecParameters::new();
        params.for_codec(CODEC_ID_AAC).with_sample_rate(*sample_rate);
        if !config.is_empty() {
            params.with_extra_data(config.clone().into_boxed_slice());
        }
        let dec = symphonia_codec_aac::AacDecoder::try_new(&params, &AudioDecoderOptions::default()).ok()?;
        Some(AudioDecoder { dec: AudioInner::Aac(Box::new(dec)), sample_rate: *sample_rate, channels: (*channels).max(1) as usize })
    }

    /// Amostras PCM intercaladas (f32), já no formato da trilha.
    pub fn decode(&mut self, s: &Sample) -> Result<Vec<f32>, String> {
        use symphonia_core::packet::PacketRef;
        use symphonia_core::units::{Duration, Timestamp};
        let dec: &mut dyn symphonia_core::codecs::audio::AudioDecoder = match &mut self.dec {
            AudioInner::Aac(d) => &mut **d,
            AudioInner::Vorbis(d) => &mut **d,
            AudioInner::Opus { dec, channels, buf } => {
                let n = dec.decode(&s.data, 5760, buf).map_err(|e| format!("opus: {e:?}"))?;
                let ch = *channels;
                let pcm = &buf[..n * ch];
                if ch <= 2 {
                    return Ok(pcm.to_vec());
                }
                // 5.1 (ordem Vorbis: FL, C, FR, SL, SR, LFE) e outros → estéreo
                let mut out = Vec::with_capacity(n * 2);
                for f in pcm.chunks(ch) {
                    let (l, r) = if ch >= 5 {
                        (0.5 * (f[0] + 0.707 * f[1] + 0.707 * f[3]), 0.5 * (f[2] + 0.707 * f[1] + 0.707 * f[4]))
                    } else {
                        (f[0], f[1])
                    };
                    out.push(l);
                    out.push(r);
                }
                return Ok(out);
            }
        };
        let packet = PacketRef::new(0, Timestamp::new(0), Duration::new(0), &s.data);
        let buf = dec.decode_ref(&packet).map_err(|e| format!("aac: {e}"))?;
        let spec = buf.spec().clone();
        self.sample_rate = spec.rate();
        self.channels = spec.channels().count().max(1);
        let mut out: Vec<f32> = Vec::new();
        buf.copy_to_vec_interleaved(&mut out);
        Ok(out)
    }

    pub fn reset(&mut self) {
        use symphonia_core::codecs::audio::AudioDecoder as _;
        match &mut self.dec {
            AudioInner::Aac(d) => d.reset(),
            AudioInner::Vorbis(d) => d.reset(),
            AudioInner::Opus { dec, .. } => dec.reset_state(),
        }
    }
}
