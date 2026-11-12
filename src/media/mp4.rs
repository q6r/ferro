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

