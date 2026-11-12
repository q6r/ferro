//! Leitor de WebM (Matroska): o contêiner do VP9/AV1 + Opus, também por
//! Media Source Extensions. Os dados chegam em pedaços (`appendBuffer`):
//! o leitor entra nos elementos grandes (`Segment`, `Cluster`) sem esperar
//! que cheguem inteiros, guarda só o que ainda não completa um elemento
//! pequeno (um bloco, as trilhas) e pula `Cues`/`SeekHead`/`Tags` sem guardar.

use std::collections::HashMap;
use std::sync::Arc;

use super::mp4::{Codec, Sample, Track};

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const CLUSTER: u32 = 0x1F43_B675;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const TIMECODE: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const BLOCK_DURATION: u32 = 0x9B;
const REFERENCE_BLOCK: u32 = 0xFB;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const DEFAULT_DURATION: u32 = 0x23_E383;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const AUDIO: u32 = 0xE1;
const SAMPLING_FREQUENCY: u32 = 0xB5;
const CHANNELS: u32 = 0x9F;

/// Começo de todo arquivo WebM/Matroska (cabeçalho EBML).
pub fn is_webm(data: &[u8]) -> bool {
    data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
}

/// Número de tamanho variável. `Ok(None)`: faltam bytes. Devolve (valor,
/// bytes, todos os bits de valor ligados = tamanho "desconhecido").
fn vint(b: &[u8], keep_marker: bool) -> Result<Option<(u64, usize, bool)>, String> {
    let Some(&first) = b.first() else { return Ok(None) };
    if first == 0 {
        return Err("WebM: número de tamanho variável inválido".into());
    }
    let len = first.leading_zeros() as usize + 1;
    if b.len() < len {
        return Ok(None);
    }
    // bits de valor do primeiro byte (com 8 bytes, nenhum: 0xFF >> 8 em u8 estouraria)
    let mask = (0xFFu16 >> len) as u8;
    let mut v: u64 = if keep_marker { first as u64 } else { (first & mask) as u64 };
    let mut all_ones = (first | !mask) == 0xFF;
    for &x in &b[1..len] {
        v = (v << 8) | x as u64;
        all_ones &= x == 0xFF;
    }
    Ok(Some((v, len, all_ones)))
}

/// Cabeçalho de elemento: (id, tamanho do cabeçalho, tamanho do conteúdo;
/// None = desconhecido).
fn header(b: &[u8]) -> Result<Option<(u32, usize, Option<u64>)>, String> {
    let Some((id, il, _)) = vint(b, true)? else { return Ok(None) };
    if il > 4 {
        return Err("WebM: ID de elemento inválido".into());
    }
    let Some((size, sl, unknown)) = vint(&b[il..], false)? else { return Ok(None) };
    Ok(Some((id as u32, il + sl, (!unknown).then_some(size))))
}

/// Filhos de um elemento já completo.
fn children(mut b: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    while let Ok(Some((id, hl, Some(size)))) = header(b) {
        let end = hl.saturating_add(size as usize);
        if end > b.len() {
            break;
        }
        out.push((id, &b[hl..end]));
        b = &b[end..];
    }
    out
}

fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0u64, |v, &x| (v << 8) | x as u64)
}

fn float(b: &[u8]) -> f64 {
    match b.len() {
        4 => f32::from_be_bytes(b.try_into().unwrap()) as f64,
        8 => f64::from_be_bytes(b.try_into().unwrap()),
        _ => 0.0,
    }
}

/// Duração de um pacote Opus, pelo byte de configuração (RFC 6716, 3.1).
pub fn opus_duration(p: &[u8]) -> Option<f64> {
    let toc = *p.first()?;
    let config = toc >> 3;
    let frame_ms = match config {
        0..=11 => [10.0, 20.0, 40.0, 60.0][(config % 4) as usize],
        12..=15 => [10.0, 20.0][(config % 2) as usize],
        _ => [2.5, 5.0, 10.0, 20.0][(config % 4) as usize],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (*p.get(1)? & 0x3F) as u32,
    };
    Some(frames as f64 * frame_ms / 1000.0)
}

/// Leitor incremental.
pub struct Webm {
    pending: Vec<u8>,
    /// Bytes que ainda faltam de um elemento que está sendo pulado.
    skip: u64,
    /// Segundos por unidade de tempo (TimecodeScale; padrão 1 ms).
    scale: f64,
    cluster_tc: i64,
    /// Duração padrão por trilha (segundos), quando o arquivo informa.
    default_dur: HashMap<u32, f64>,
    /// Último tempo visto por trilha (para estimar a duração do bloco).
    last: HashMap<u32, (f64, f64)>,
}

impl Default for Webm {
    fn default() -> Self {
        Webm { pending: Vec::new(), skip: 0, scale: 1e-3, cluster_tc: 0, default_dur: HashMap::new(), last: HashMap::new() }
    }
}

impl Webm {
    /// Acrescenta bytes; atualiza as trilhas e devolve as amostras completas.
    pub fn push(&mut self, data: &[u8], tracks: &mut Vec<Track>) -> Result<Vec<Sample>, String> {
        self.pending.extend_from_slice(data);
        let mut out = Vec::new();
        let mut pos = 0usize;
        loop {
            if self.skip > 0 {
                let n = (self.skip as usize).min(self.pending.len() - pos);
                pos += n;
                self.skip -= n as u64;
                if self.skip > 0 {
                    break;
                }
                continue;
            }
            let Some((id, hl, size)) = header(&self.pending[pos..])? else { break };
            if matches!(id, SEGMENT | CLUSTER) {
                // entra: os filhos vêm em seguida, um a um
                pos += hl;
                continue;
            }
            let Some(size) = size else {
                return Err(format!("WebM: elemento {id:#x} de tamanho desconhecido"));
            };
            match id {
                EBML | INFO | TRACKS | TIMECODE | SIMPLE_BLOCK | BLOCK_GROUP => {
                    if size > 512 << 20 {
                        return Err("WebM: elemento grande demais".into());
                    }
                    let end = pos + hl + size as usize;
                    if self.pending.len() < end {
                        break;
                    }
                    let body = self.pending[pos + hl..end].to_vec();
                    pos = end;
                    match id {
                        INFO => self.read_info(&body),
                        TRACKS => *tracks = self.read_tracks(&body),
                        TIMECODE => self.cluster_tc = uint(&body) as i64,
                        SIMPLE_BLOCK => self.read_block(&body, None, None, tracks, &mut out),
                        BLOCK_GROUP => {
                            let kids = children(&body);
                            let dur = kids.iter().find(|(i, _)| *i == BLOCK_DURATION).map(|(_, b)| uint(b));
                            let key = !kids.iter().any(|(i, _)| *i == REFERENCE_BLOCK);
                            if let Some((_, block)) = kids.iter().find(|(i, _)| *i == BLOCK) {
                                self.read_block(block, Some(key), dur, tracks, &mut out);
                            }
                        }
                        _ => {}
                    }
                }
                // SeekHead, Cues, Tags, Void, CRC, Chapters...: pula sem guardar
                _ => {
                    pos += hl;
                    self.skip = size;
                }
            }
        }
        self.pending.drain(..pos);
        Ok(out)
    }

    fn read_info(&mut self, b: &[u8]) {
        for (id, v) in children(b) {
            if id == TIMECODE_SCALE {
                let ns = uint(v);
                if ns > 0 {
                    self.scale = ns as f64 / 1e9;
                }
            }
        }
    }

    fn read_tracks(&mut self, b: &[u8]) -> Vec<Track> {
        let mut out = Vec::new();
        for (id, entry) in children(b) {
            if id != TRACK_ENTRY {
                continue;
            }
            let (mut num, mut kind, mut codec_id, mut private) = (0u32, 0u64, String::new(), Vec::new());
            let (mut width, mut height, mut rate, mut channels) = (0u32, 0u32, 48000.0f64, 2u16);
            for (k, v) in children(entry) {
                match k {
                    TRACK_NUMBER => num = uint(v) as u32,
                    TRACK_TYPE => kind = uint(v),
                    CODEC_ID => codec_id = String::from_utf8_lossy(v).trim_end_matches('\0').to_string(),
                    CODEC_PRIVATE => private = v.to_vec(),
                    DEFAULT_DURATION => {
                        self.default_dur.insert(num, uint(v) as f64 / 1e9);
                    }
                    VIDEO => {
                        for (k2, v2) in children(v) {
                            match k2 {
                                PIXEL_WIDTH => width = uint(v2) as u32,
                                PIXEL_HEIGHT => height = uint(v2) as u32,
                                _ => {}
                            }
                        }
                    }
                    AUDIO => {
                        for (k2, v2) in children(v) {
                            match k2 {
                                SAMPLING_FREQUENCY => rate = float(v2),
                                CHANNELS => channels = uint(v2) as u16,
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            // DefaultDuration pode vir antes do TrackNumber: reacerta a chave
            if let Some(d) = self.default_dur.remove(&0) {
                self.default_dur.insert(num, d);
            }
            let codec = match codec_id.as_str() {
                "V_VP9" => Codec::Vp9,
                "V_VP8" => Codec::Vp8,
                "A_VORBIS" => Codec::Vorbis { headers: private },
                "V_AV1" => Codec::Av1 { config: private.get(4..).unwrap_or(&[]).to_vec() },
                "V_MPEG4/ISO/AVC" => Codec::Avc { config: private },
                "A_OPUS" => Codec::Opus { head: private },
                "A_AAC" => Codec::Aac { config: private, channels, sample_rate: rate as u32 },
                other => Codec::Other(other.to_string()),
            };
            let _ = kind;
            out.push(Track {
                id: num,
                timescale: 1000,
                codec,
                width,
                height,
                default_duration: 0,
                default_size: 0,
                default_flags: 0,
            });
        }
        out
    }

    /// Bloco (SimpleBlock ou Block de um BlockGroup): número da trilha, tempo
    /// relativo ao cluster, flags e um ou mais quadros (lacing).
    fn read_block(&mut self, b: &[u8], key: Option<bool>, dur: Option<u64>, tracks: &[Track], out: &mut Vec<Sample>) {
        let Ok(Some((track, tl, _))) = vint(b, false) else { return };
        let track = track as u32;
        if b.len() < tl + 3 {
            return;
        }
        let rel = i16::from_be_bytes([b[tl], b[tl + 1]]) as i64;
        let flags = b[tl + 2];
        let key = key.unwrap_or(flags & 0x80 != 0);
        let data = &b[tl + 3..];
        let Some(frames) = unlace(data, (flags >> 1) & 3) else { return };
        let Some(t) = tracks.iter().find(|t| t.id == track) else { return };
        let pts = (self.cluster_tc + rel) as f64 * self.scale;
        let n = frames.len().max(1) as f64;
        let is_opus = matches!(t.codec, Codec::Opus { .. });
        let video = t.is_video();
        for (i, f) in frames.iter().enumerate() {
            let est = || {
                // a diferença para o bloco anterior, ou um valor típico
                self.last.get(&track).map(|&(_, d)| d).unwrap_or(if video { 1.0 / 30.0 } else { 0.02 })
            };
            let d = if let Some(d) = dur {
                d as f64 * self.scale / n
            } else if let Some(&d) = self.default_dur.get(&track) {
                d
            } else if is_opus {
                opus_duration(f).unwrap_or_else(est)
            } else {
                est()
            };
            let p = pts + i as f64 * d;
            if let Some(&(prev, _)) = self.last.get(&track)
                && p > prev
                && dur.is_none()
            {
                let delta = p - prev;
                self.last.insert(track, (p, delta));
            } else {
                self.last.insert(track, (p, d));
            }
            out.push(Sample {
                track,
                dts: p,
                pts: p,
                duration: d,
                key: key && i == 0 || !video,
                data: Arc::from(*f),
            });
        }
    }
}

/// Separa os quadros de um bloco: sem lacing (0), Xiph (1), fixo (2) ou EBML (3).
fn unlace(data: &[u8], lacing: u8) -> Option<Vec<&[u8]>> {
    if lacing == 0 {
        return Some(vec![data]);
    }
    let count = *data.first()? as usize + 1;
    let mut i = 1usize;
    let mut sizes = Vec::with_capacity(count);
    match lacing {
        1 => {
            for _ in 0..count - 1 {
                let mut s = 0usize;
                loop {
                    let x = *data.get(i)?;
                    i += 1;
                    s += x as usize;
                    if x != 255 {
                        break;
                    }
                }
                sizes.push(s);
            }
        }
        2 => {
            let each = (data.len() - 1) / count;
            sizes = vec![each; count - 1];
        }
        _ => {
            let (first, l, _) = vint(&data[i..], false).ok()??;
            i += l;
            sizes.push(first as usize);
            let mut prev = first as i64;
            for _ in 1..count - 1 {
                let (raw, l, _) = vint(&data[i..], false).ok()??;
                i += l;
                // diferença com sinal: valor - (2^(7l-1) - 1)
                let bias = (1i64 << (7 * l - 1)) - 1;
                prev += raw as i64 - bias;
                if prev < 0 {
                    return None;
                }
                sizes.push(prev as usize);
            }
        }
    }
    let mut frames = Vec::with_capacity(count);
    for s in sizes {
        frames.push(data.get(i..i + s)?);
        i += s;
    }
    frames.push(data.get(i..)?);
    Some(frames)
}

/// Um arquivo WebM inteiro (`<video src>`).
pub fn demux_file(data: &[u8]) -> Result<(Vec<Track>, Vec<Sample>), String> {
    let mut w = Webm::default();
    let mut tracks = Vec::new();
    let samples = w.push(data, &mut tracks)?;
    Ok((tracks, samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.push(0x01); // tamanho em 8 bytes (marcador no primeiro)
        v.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn le_trilhas_e_blocos_em_pedacos() {
        let entry = [
            el(&[0xD7], &[1]),
            el(&[0x83], &[2]),
            el(&[0x86], b"A_OPUS"),
            el(&[0x63, 0xA2], b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0"),
        ]
        .concat();
        let tracks = el(&[0x16, 0x54, 0xAE, 0x6B], &el(&[0xAE], &entry));
        // bloco simples: trilha 1, tempo +20, chave, pacote Opus de 20 ms (config 1 SILK NB 20 ms)
        let block = el(&[0xA3], &[0x81, 0x00, 0x14, 0x80, 0x08, 0xAA, 0xBB]);
        let cluster_body = [el(&[0xE7], &[0x03, 0xE8]), block].concat();
        let mut file = el(&[0x1A, 0x45, 0xDF, 0xA3], &[]);
        file.extend([0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // Segment de tamanho desconhecido
        file.extend(tracks);
        file.extend([0x1F, 0x43, 0xB6, 0x75, 0xFF]); // Cluster de tamanho desconhecido
        file.extend(cluster_body);
        // entrega em pedaços de 3 bytes
        let mut w = Webm::default();
        let mut t = Vec::new();
        let mut samples = Vec::new();
        for chunk in file.chunks(3) {
            samples.extend(w.push(chunk, &mut t).unwrap());
        }
        assert_eq!(t.len(), 1);
        assert!(matches!(t[0].codec, Codec::Opus { .. }));
        assert_eq!(samples.len(), 1);
        assert!((samples[0].pts - 1.02).abs() < 1e-9);
        assert!((samples[0].duration - 0.02).abs() < 1e-9);
        assert_eq!(&*samples[0].data, &[0x08, 0xAA, 0xBB]);
    }

    #[test]
    fn lacing_xiph_e_ebml() {
        let x = unlace(&[2, 3, 1, 9, 9, 9, 8, 7, 7], 1).unwrap();
        assert_eq!(x, vec![&[9, 9, 9][..], &[8][..], &[7, 7][..]]);
        // EBML: 3 quadros, primeiro 2 bytes, segundo 2 + (0x5F - 63 = 32)... usa diferença 0
        let e = unlace(&[2, 0x82, 0xBF, 1, 1, 2, 2, 3], 3).unwrap();
        assert_eq!(e, vec![&[1, 1][..], &[2, 2][..], &[3][..]]);
    }
}
