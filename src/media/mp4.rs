//! Leitor de MP4 fragmentado (fMP4/CMAF), o formato que o YouTube entrega
//! por Media Source Extensions: um segmento de inicialização (`moov`, com a
//! descrição das trilhas) e segmentos de mídia (`moof` + `mdat`, com as
//! amostras). Os dados chegam em pedaços arbitrários (`appendBuffer`), então
//! o leitor guarda o que ainda não forma uma caixa completa.

use std::sync::Arc;

/// Codec de uma trilha.
#[derive(Clone, Debug, PartialEq)]
pub enum Codec {
    /// AV1: `config` são os OBUs de configuração do `av1C` (sequence header).
    Av1 { config: Vec<u8> },
    /// H.264: o registro `avcC` inteiro.
    Avc { config: Vec<u8> },
    /// AAC: o AudioSpecificConfig do `esds`.
    Aac { config: Vec<u8>, channels: u16, sample_rate: u32 },
    Other(String),
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: u32,
    pub timescale: u32,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    /// Valores padrão do `trex` (duração, tamanho, flags).
    pub default_duration: u32,
    pub default_size: u32,
    pub default_flags: u32,
}

impl Track {
    pub fn is_video(&self) -> bool {
        matches!(self.codec, Codec::Av1 { .. } | Codec::Avc { .. })
    }
}

/// Uma amostra (quadro de vídeo ou bloco de áudio), em segundos.
#[derive(Clone, Debug)]
pub struct Sample {
    pub track: u32,
    pub dts: f64,
    pub pts: f64,
    pub duration: f64,
    pub key: bool,
    pub data: Arc<[u8]>,
}

/// Leitor incremental.
#[derive(Default)]
pub struct Demuxer {
    pending: Vec<u8>,
    pub tracks: Vec<Track>,
    /// `moof` lido, esperando o `mdat` (bytes do moof, posição no fluxo).
    moof: Option<Vec<u8>>,
}

fn be16(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2).map_or(0, |s| u16::from_be_bytes([s[0], s[1]]))
}

fn be32(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4).map_or(0, |s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be64(b: &[u8], i: usize) -> u64 {
    b.get(i..i + 8).map_or(0, |s| u64::from_be_bytes(s.try_into().unwrap()))
}

/// Caixas filhas de um trecho: (tipo, conteúdo sem o cabeçalho).
fn boxes(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= b.len() {
        let size32 = be32(b, i) as usize;
        let ty: [u8; 4] = b[i + 4..i + 8].try_into().unwrap();
        let (size, hdr) = match size32 {
            0 => (b.len() - i, 8),
            1 => (be64(b, i + 8) as usize, 16),
            s => (s, 8),
        };
        if size < hdr || i + size > b.len() {
            break;
        }
        out.push((ty, &b[i + hdr..i + size]));
        i += size;
    }
    out
}

fn child<'a>(b: &'a [u8], ty: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(b).into_iter().find(|(t, _)| t == ty).map(|(_, c)| c)
}

/// Tamanho (variável) de um descritor MPEG-4 (esds).
fn desc_len(b: &[u8], i: &mut usize) -> usize {
    let mut n = 0usize;
    for _ in 0..4 {
        let Some(&c) = b.get(*i) else { break };
        *i += 1;
        n = (n << 7) | (c & 0x7f) as usize;
        if c & 0x80 == 0 {
            break;
        }
    }
    n
}

/// AudioSpecificConfig de dentro do `esds`.
fn esds_config(esds: &[u8]) -> Option<Vec<u8>> {
    // versão/flags (4), depois ES_Descriptor (tag 3)
    let b = esds.get(4..)?;
    let mut i = 0;
    if *b.get(i)? != 3 {
        return None;
    }
    i += 1;
    let _ = desc_len(b, &mut i);
    i += 2; // ES_ID
    let flags = *b.get(i)?;
    i += 1;
    if flags & 0x80 != 0 {
        i += 2;
    }
    if flags & 0x40 != 0 {
        let n = *b.get(i)? as usize;
        i += 1 + n;
    }
    if flags & 0x20 != 0 {
        i += 2;
    }
    // DecoderConfigDescriptor (tag 4)
    if *b.get(i)? != 4 {
        return None;
    }
    i += 1;
    let _ = desc_len(b, &mut i);
    i += 13;
    // DecoderSpecificInfo (tag 5)
    if *b.get(i)? != 5 {
        return None;
    }
    i += 1;
    let n = desc_len(b, &mut i);
    b.get(i..i + n).map(|s| s.to_vec())
}

/// MP4 comum (não fragmentado, o de `<video src="x.mp4">`): as trilhas do
/// `moov` e todas as amostras, montadas pelas tabelas do `stbl`.
pub fn demux_file(data: &[u8]) -> Result<(Vec<Track>, Vec<Sample>), String> {
    let top = boxes(data);
    let moov = top.iter().find(|(t, _)| t == b"moov").map(|(_, b)| *b).ok_or("MP4 sem moov")?;
    let mut d = Demuxer::new();
    d.read_moov(moov)?;
    let mut samples = Vec::new();
    for (ty, trak) in boxes(moov) {
        if &ty != b"trak" {
            continue;
        }
        let tkhd = child(trak, b"tkhd").ok_or("trak sem tkhd")?;
        let id = if tkhd.first() == Some(&1) { be32(tkhd, 20) } else { be32(tkhd, 12) };
        let Some(track) = d.track(id) else { continue };
        if matches!(track.codec, Codec::Other(_)) {
            continue;
        }
        let ts = track.timescale.max(1) as f64;
        let Some(stbl) = child(trak, b"mdia").and_then(|m| child(m, b"minf")).and_then(|m| child(m, b"stbl")) else { continue };
        // tamanhos
        let sizes: Vec<u32> = match (child(stbl, b"stsz"), child(stbl, b"stz2")) {
            (Some(stsz), _) => {
                let fixed = be32(stsz, 4);
                let n = be32(stsz, 8) as usize;
                if fixed != 0 { vec![fixed; n] } else { (0..n).map(|i| be32(stsz, 12 + i * 4)).collect() }
            }
            (None, Some(stz2)) => {
                let bits = stz2.get(7).copied().unwrap_or(16);
                let n = be32(stz2, 8) as usize;
                (0..n)
                    .map(|i| match bits {
                        8 => stz2.get(12 + i).copied().unwrap_or(0) as u32,
                        4 => {
                            let b = stz2.get(12 + i / 2).copied().unwrap_or(0);
                            (if i % 2 == 0 { b >> 4 } else { b & 15 }) as u32
                        }
                        _ => be16(stz2, 12 + i * 2) as u32,
                    })
                    .collect()
            }
            _ => continue,
        };
        // posição de cada pedaço (chunk)
        let chunks: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
            (0..be32(stco, 4) as usize).map(|i| be32(stco, 8 + i * 4) as u64).collect()
        } else if let Some(co64) = child(stbl, b"co64") {
            (0..be32(co64, 4) as usize).map(|i| be64(co64, 8 + i * 8)).collect()
        } else {
            continue;
        };
        // amostras por pedaço: (primeiro pedaço, amostras por pedaço)
        let stsc = child(stbl, b"stsc").ok_or("stbl sem stsc")?;
        let runs: Vec<(u32, u32)> = (0..be32(stsc, 4) as usize).map(|i| (be32(stsc, 8 + i * 12), be32(stsc, 12 + i * 12))).collect();
        // durações (stts) e deslocamentos de composição (ctts)
        let stts = child(stbl, b"stts").ok_or("stbl sem stts")?;
        let mut durations = Vec::with_capacity(sizes.len());
        for i in 0..be32(stts, 4) as usize {
            let (count, delta) = (be32(stts, 8 + i * 8), be32(stts, 12 + i * 8));
            for _ in 0..count {
                durations.push(delta);
            }
        }
        let mut offsets: Vec<i64> = Vec::new();
        if let Some(ctts) = child(stbl, b"ctts") {
            let v1 = ctts.first() == Some(&1);
            for i in 0..be32(ctts, 4) as usize {
                let count = be32(ctts, 8 + i * 8);
                let raw = be32(ctts, 12 + i * 8);
                let off = if v1 { raw as i32 as i64 } else { raw as i64 };
                for _ in 0..count {
                    offsets.push(off);
                }
            }
        }
        // quadros-chave (sem stss: todos são)
        let sync: Option<std::collections::HashSet<u32>> =
            child(stbl, b"stss").map(|s| (0..be32(s, 4) as usize).map(|i| be32(s, 8 + i * 4)).collect());
        // percorre pedaços e amostras
        let mut sample = 0usize;
        let mut dts: u64 = 0;
        for (ci, &chunk_pos) in chunks.iter().enumerate() {
            let chunk_no = ci as u32 + 1;
            let per = runs.iter().rev().find(|(first, _)| *first <= chunk_no).map_or(0, |r| r.1);
            let mut pos = chunk_pos as usize;
            for _ in 0..per {
                let Some(&size) = sizes.get(sample) else { break };
                let size = size as usize;
                let dur = durations.get(sample).copied().unwrap_or(0) as u64;
                let cto = offsets.get(sample).copied().unwrap_or(0);
                if let Some(bytes) = data.get(pos..pos + size) {
                    samples.push(Sample {
                        track: id,
                        dts: dts as f64 / ts,
                        pts: (dts as i64 + cto) as f64 / ts,
                        duration: dur as f64 / ts,
                        key: sync.as_ref().is_none_or(|s| s.contains(&(sample as u32 + 1))),
                        data: Arc::from(bytes),
                    });
                }
                pos += size;
                dts += dur;
                sample += 1;
            }
        }
    }
    Ok((d.tracks, samples))
}

