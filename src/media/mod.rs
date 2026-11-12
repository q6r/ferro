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

impl Playback {
    fn new() -> Playback {
        Playback {
            source: None,
            paused: true,
            seeking: false,
            ended: false,
            waiting: false,
            ready_state: 0,
            rate: 1.0,
            volume: 1.0,
            muted: false,
            base: 0.0,
            since: Instant::now(),
            last_timeupdate: Instant::now(),
            width: 0,
            height: 0,
            pipeline: player::Pipeline::new(),
        }
    }

    /// Tempo atual da mídia.
    pub fn current_time(&self) -> f64 {
        if self.paused || self.waiting || self.seeking || self.ended {
            self.base
        } else {
            self.base + self.since.elapsed().as_secs_f64() * self.rate
        }
    }

    /// Recomeça o relógio em `t` (troca de velocidade).
    pub fn seek_base(&mut self, t: f64) {
        self.base = t;
        self.since = Instant::now();
    }

    fn freeze(&mut self) {
        self.base = self.current_time();
        self.since = Instant::now();
    }
}

/// Tudo o que é mídia numa página.
#[derive(Default)]
pub struct Media {
    seq: u32,
    pub sources: HashMap<u32, MediaSource>,
    pub buffers: HashMap<u32, SourceBuffer>,
    pub players: HashMap<NodeId, Playback>,
    /// Pasta para gravar o que chega por appendBuffer (FERRO_MIDIA_DUMP).
    dump: Option<std::path::PathBuf>,
    /// Elementos com quadro novo desde a última consulta.
    pub new_frames: std::collections::HashSet<NodeId>,
    /// Destino dos quadros (a interface), passado aos pipelines.
    pub frame_sink: Option<player::FrameSink>,
}

impl Media {
    pub fn new() -> Media {
        Media {
            dump: std::env::var_os("FERRO_MIDIA_DUMP").map(Into::into),
            ..Media::default()
        }
    }

    fn next(&mut self) -> u32 {
        self.seq += 1;
        self.seq
    }

    pub fn new_source(&mut self) -> u32 {
        let id = self.next();
        self.sources.insert(id, MediaSource { duration: f64::NAN, buffers: Vec::new(), ended: false });
        id
    }

    pub fn add_buffer(&mut self, source: u32, mime: &str) -> Result<u32, String> {
        if !is_type_supported(mime) {
            return Err(format!("Failed to execute 'addSourceBuffer' on 'MediaSource': The type provided ('{mime}') is unsupported."));
        }
        let id = self.next();
        self.buffers.insert(
            id,
            SourceBuffer {
                source,
                mime: mime.to_string(),
                demux: Demuxer::new(),
                samples: HashMap::new(),
                timestamp_offset: 0.0,
            },
        );
        if let Some(s) = self.sources.get_mut(&source) {
            s.buffers.push(id);
        }
        Ok(id)
    }

    pub fn append(&mut self, id: u32, data: &[u8], offset: f64) -> Result<(), String> {
        if let Some(dir) = &self.dump {
            use std::io::Write;
            let path = dir.join(format!("sb{id}.mp4"));
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = f.write_all(data);
            }
        }
        let sb = self.buffers.get_mut(&id).ok_or("SourceBuffer removido")?;
        sb.timestamp_offset = offset;
        let samples = sb.demux.push(data)?;
        for mut s in samples {
            s.pts += offset;
            s.dts += offset;
            let shared = sb.samples.entry(s.track).or_default().clone();
            let mut list = shared.lock().unwrap();
            // substitui amostras no mesmo intervalo (reenvio do mesmo trecho)
            if let Some(last) = list.last()
                && s.dts <= last.dts
            {
                list.retain(|x| (x.dts - s.dts).abs() > 1e-6);
                let pos = list.partition_point(|x| x.dts < s.dts);
                list.insert(pos, s);
            } else {
                list.push(s);
            }
        }
        Ok(())
    }

