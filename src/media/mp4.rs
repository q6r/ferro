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

