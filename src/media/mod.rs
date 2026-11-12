//! Mídia: Media Source Extensions (o player JS entrega pedaços de MP4
//! fragmentado), o relógio de reprodução de cada `<video>`/`<audio>` e os
//! eventos que ele gera. Decodificação e apresentação ficam em `player.rs`.

pub mod decode;
pub mod mp4;
pub mod player;

use std::collections::HashMap;
use std::time::Instant;

use crate::dom::NodeId;
use mp4::{Codec, Demuxer};
use player::SharedSamples;

/// Tipos que o Ferro sabe tocar (o player do site escolhe o formato por isto).
/// Parâmetros além de `codecs` são ignorados, como no Chrome.
pub fn is_type_supported(mime: &str) -> bool {
    let mut parts = mime.split(';');
    let ty = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let mut codecs = None;
    for p in parts {
        if let Some((k, v)) = p.split_once('=')
            && k.trim().eq_ignore_ascii_case("codecs")
        {
            codecs = Some(v.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase());
        }
    }
    let ok_codec = |c: &str| {
        let c = c.trim();
        // AV1 8 bits; H.264 Baseline (42) e Main (4d); AAC
        (c.starts_with("av01.0.") && !c.contains(".10."))
            || c.starts_with("avc1.42")
            || c.starts_with("avc1.4d")
            || c.starts_with("mp4a.40.2")
            || c.starts_with("mp4a.40.5")
            || c == "mp4a.40"
    };
    match ty.as_str() {
        "video/mp4" | "audio/mp4" => codecs.is_none_or(|c| !c.is_empty() && c.split(',').all(ok_codec)),
        _ => false,
    }
}

pub struct SourceBuffer {
    pub source: u32,
    pub mime: String,
    pub demux: Demuxer,
    /// Amostras por trilha, em ordem de decodificação (compartilhadas com as
    /// threads que decodificam).
    pub samples: HashMap<u32, SharedSamples>,
    pub timestamp_offset: f64,
}

impl SourceBuffer {
