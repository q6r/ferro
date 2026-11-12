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
    /// Intervalos [início, fim) com dados (as amostras de uma trilha).
    pub fn buffered(&self) -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = Vec::new();
        // a trilha com menos dados define (vídeo e áudio no mesmo buffer: raro no YouTube)
        let mut ranges_per_track: Vec<Vec<(f64, f64)>> = Vec::new();
        for list in self.samples.values() {
            let list = list.lock().unwrap();
            let mut iv: Vec<(f64, f64)> = list.iter().map(|s| (s.pts, s.pts + s.duration.max(0.001))).collect();
            iv.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut merged: Vec<(f64, f64)> = Vec::new();
            for (s, e) in iv {
                match merged.last_mut() {
                    // tolera buracos pequenos entre amostras
                    Some(last) if s <= last.1 + 0.15 => last.1 = last.1.max(e),
                    _ => merged.push((s, e)),
                }
            }
            ranges_per_track.push(merged);
        }
        if let Some(first) = ranges_per_track.first() {
            out = first.clone();
            for other in &ranges_per_track[1..] {
                out = intersect(&out, other);
            }
        }
        out
    }
}

fn intersect(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(s1, e1) in a {
        for &(s2, e2) in b {
            let (s, e) = (s1.max(s2), e1.min(e2));
            if e > s {
                out.push((s, e));
            }
        }
    }
    out
}

pub struct MediaSource {
    pub duration: f64,
    pub buffers: Vec<u32>,
    pub ended: bool,
}

/// Estado de reprodução de um elemento de mídia.
pub struct Playback {
    pub source: Option<u32>,
    pub paused: bool,
    pub seeking: bool,
    pub ended: bool,
    /// Esperando dados (tocando, mas sem o que mostrar).
    pub waiting: bool,
    pub ready_state: u8,
    pub rate: f64,
    pub volume: f64,
    pub muted: bool,
    /// Tempo da mídia em `since` (o relógio anda a partir daí quando toca).
    base: f64,
    since: Instant,
    last_timeupdate: Instant,
    pub width: u32,
    pub height: u32,
    /// Decodificadores e o quadro atual.
    pub pipeline: player::Pipeline,
}

