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

    /// Um arquivo MP4 inteiro (`<video src>`): vira uma fonte já completa.
    pub fn load_file(&mut self, data: &[u8]) -> Result<u32, String> {
        let (tracks, samples) = mp4::demux_file(data)?;
        if !tracks.iter().any(|t| !matches!(t.codec, Codec::Other(_))) {
            return Err("nenhuma trilha em formato suportado".into());
        }
        let source = self.new_source();
        let id = self.next();
        let mut demux = Demuxer::new();
        demux.tracks = tracks;
        let mut map: HashMap<u32, SharedSamples> = HashMap::new();
        let mut end: f64 = 0.0;
        // começa do zero (atraso de composição dos quadros B, listas de edição)
        let first = samples.iter().map(|s| s.pts).fold(f64::INFINITY, f64::min);
        let shift = if first.is_finite() && first > 0.0 { first } else { 0.0 };
        let mut samples = samples;
        for s in samples.iter_mut() {
            s.pts -= shift;
            s.dts -= shift;
        }
        if std::env::var_os("FERRO_MIDIA_LOG").is_some() {
            eprintln!("[mídia] arquivo: {} amostras, começa em {first:.3} s", samples.len());
        }
        for s in samples {
            end = end.max(s.pts + s.duration);
            map.entry(s.track).or_default().lock().unwrap().push(s);
        }
        for list in map.values() {
            list.lock().unwrap().sort_by(|a, b| a.dts.total_cmp(&b.dts));
        }
        self.buffers.insert(id, SourceBuffer { source, mime: "video/mp4".into(), demux, samples: map, timestamp_offset: 0.0 });
        if let Some(s) = self.sources.get_mut(&source) {
            s.buffers.push(id);
            s.duration = end;
            s.ended = true;
        }
        if std::env::var_os("FERRO_MIDIA_LOG").is_some() {
            eprintln!("[mídia] arquivo: duração {end:.2} s, dados em {:?}", self.source_buffered(source));
        }
        Ok(source)
    }

    pub fn remove(&mut self, id: u32, start: f64, end: f64) {
        if let Some(sb) = self.buffers.get_mut(&id) {
            for list in sb.samples.values() {
                list.lock().unwrap().retain(|s| s.pts < start || s.pts >= end);
            }
        }
    }

    /// Intervalos com dados em todos os buffers da fonte.
    pub fn source_buffered(&self, source: u32) -> Vec<(f64, f64)> {
        let Some(ms) = self.sources.get(&source) else { return Vec::new() };
        let mut out: Option<Vec<(f64, f64)>> = None;
        for id in &ms.buffers {
            let Some(sb) = self.buffers.get(id) else { continue };
            if sb.demux.tracks.is_empty() {
                continue;
            }
            let b = sb.buffered();
            out = Some(match out {
                None => b,
                Some(prev) => intersect(&prev, &b),
            });
        }
        out.unwrap_or_default()
    }

    fn has_metadata(&self, source: u32) -> bool {
        self.sources.get(&source).is_some_and(|ms| {
            !ms.buffers.is_empty() && ms.buffers.iter().all(|id| self.buffers.get(id).is_some_and(|b| !b.demux.tracks.is_empty()))
        })
    }

    /// Tamanho do vídeo (da primeira trilha de vídeo).
    fn video_size(&self, source: u32) -> (u32, u32) {
        let Some(ms) = self.sources.get(&source) else { return (0, 0) };
        for id in &ms.buffers {
            if let Some(sb) = self.buffers.get(id)
                && let Some(t) = sb.demux.tracks.iter().find(|t| t.is_video())
            {
                return (t.width, t.height);
            }
        }
        (0, 0)
    }

    fn new_playback(&self, el: NodeId) -> Playback {
        let mut p = Playback::new();
        p.pipeline.node = el;
        p.pipeline.sink = self.frame_sink.clone();
        p
    }

    pub fn attach(&mut self, el: NodeId, source: u32) {
        let mut p = self.new_playback(el);
        p.source = Some(source);
        self.players.insert(el, p);
    }

    pub fn player(&mut self, el: NodeId) -> &mut Playback {
        if !self.players.contains_key(&el) {
            let p = self.new_playback(el);
            self.players.insert(el, p);
        }
        self.players.get_mut(&el).unwrap()
    }

    pub fn play(&mut self, el: NodeId) -> Vec<&'static str> {
        let p = self.player(el);
        let mut ev = Vec::new();
        if p.ended {
            p.ended = false;
            p.base = 0.0;
        }
        if p.paused {
            p.paused = false;
            p.since = Instant::now();
            ev.push("play");
            if p.ready_state < 3 {
                p.waiting = true;
                ev.push("waiting");
            } else {
                ev.push("playing");
            }
        }
        ev
    }

    pub fn pause(&mut self, el: NodeId) -> Vec<&'static str> {
        let p = self.player(el);
        if p.paused {
            return Vec::new();
        }
        p.freeze();
        p.paused = true;
        let vol = if p.muted { 0.0 } else { p.volume as f32 };
        p.pipeline.sync_clock(p.base, p.rate, false, vol);
        vec!["timeupdate", "pause"]
    }

    pub fn seek(&mut self, el: NodeId, t: f64) -> Vec<&'static str> {
        let t = t.max(0.0);
        let p = self.player(el);
        p.pipeline.seek(t);
        p.base = t;
        p.since = Instant::now();
        p.seeking = true;
        p.ended = false;
        vec!["seeking"]
    }

    /// (codec, lista de amostras) da trilha de vídeo e da de áudio de uma fonte.
    #[allow(clippy::type_complexity)]
    fn tracks_of(&self, source: u32) -> (Option<(Codec, SharedSamples)>, Option<(Codec, SharedSamples)>) {
        let (mut v, mut a) = (None, None);
        let Some(ms) = self.sources.get(&source) else { return (None, None) };
        for id in &ms.buffers {
            let Some(sb) = self.buffers.get(id) else { continue };
            for t in &sb.demux.tracks {
                let Some(list) = sb.samples.get(&t.id) else { continue };
                if t.is_video() {
                    v.get_or_insert((t.codec.clone(), list.clone()));
                } else if matches!(t.codec, Codec::Aac { .. }) {
                    a.get_or_insert((t.codec.clone(), list.clone()));
                }
            }
        }
        (v, a)
    }

    /// Liga as trilhas aos decodificadores, acerta o relógio e pega o quadro atual.
    fn advance_pipelines(&mut self) {
        let ids: Vec<NodeId> = self.players.keys().copied().collect();
        for el in ids {
            let Some(source) = self.players[&el].source else { continue };
            let (video, audio) = self.tracks_of(source);
            let p = self.players.get_mut(&el).unwrap();
            let t = p.current_time();
            let playing = !p.paused && !p.waiting && !p.seeking && !p.ended;
            let volume = if p.muted { 0.0 } else { p.volume as f32 };
            p.pipeline.set_sources(video.as_ref().map(|(c, l)| (c, l)), audio.as_ref().map(|(c, l)| (c, l)));
            p.pipeline.sync_clock(t, p.rate, playing, volume);
            if p.pipeline.present() {
                self.new_frames.insert(el);
            }
        }
    }

    /// Uma volta: atualiza o estado de cada player e devolve os eventos.
    pub fn tick(&mut self) -> Vec<(NodeId, &'static str)> {
        self.advance_pipelines();
        let mut events = Vec::new();
        let ids: Vec<NodeId> = self.players.keys().copied().collect();
        for el in ids {
            let Some(source) = self.players[&el].source else { continue };
            let meta = self.has_metadata(source);
            let ranges = self.source_buffered(source);
            let (duration, ended_stream) = self
                .sources
                .get(&source)
                .map_or((f64::NAN, false), |s| (s.duration, s.ended));
            let size = self.video_size(source);
            let p = self.players.get_mut(&el).unwrap();
            let t = p.current_time();
            let has = |x: f64| ranges.iter().any(|&(s, e)| x >= s - 0.25 && x < e);
            let near_end = duration.is_finite() && t >= duration - 0.25;
            let rs = if !meta {
                0
            } else if has(t) && (has(t + 0.5) || near_end || ended_stream) {
                4
            } else if has(t) {
                2
            } else {
                1
            };
            let old = p.ready_state;
            if old < 1 && rs >= 1 {
                p.width = size.0;
                p.height = size.1;
                if duration.is_finite() {
                    events.push((el, "durationchange"));
                }
                events.push((el, "loadedmetadata"));
                if size.0 > 0 {
                    events.push((el, "resize"));
                }
            }
            if old < 2 && rs >= 2 {
                events.push((el, "loadeddata"));
            }
            if old < 3 && rs >= 3 {
                events.push((el, "canplay"));
            }
            if old < 4 && rs >= 4 {
                events.push((el, "canplaythrough"));
            }
            p.ready_state = rs;
            if p.seeking && rs >= 2 {
                p.seeking = false;
                p.since = Instant::now();
                events.push((el, "timeupdate"));
                events.push((el, "seeked"));
            }
            if !p.paused && !p.ended {
                if p.waiting && rs >= 3 {
                    p.waiting = false;
                    p.since = Instant::now();
                    events.push((el, "playing"));
                } else if !p.waiting && rs < 3 && !(ended_stream && near_end) {
                    p.freeze();
                    p.waiting = true;
                    events.push((el, "waiting"));
                }
                if ended_stream && duration.is_finite() && p.current_time() >= duration - 0.05 {
                    p.freeze();
                    p.base = duration;
                    p.ended = true;
                    p.paused = true;
                    events.push((el, "timeupdate"));
                    events.push((el, "pause"));
                    events.push((el, "ended"));
                } else if p.last_timeupdate.elapsed().as_millis() >= 250 && !p.waiting {
                    p.last_timeupdate = Instant::now();
                    events.push((el, "timeupdate"));
                }
            }
        }
        events
    }

    /// Algum player tocando (o laço da aba não pode dormir muito).
    pub fn active(&self) -> bool {
        self.players.values().any(|p| !p.paused || p.seeking || (p.source.is_some() && p.ready_state < 4))
    }

    /// Informações das trilhas (para depuração).
    pub fn describe(&self, source: u32) -> String {
        let Some(ms) = self.sources.get(&source) else { return String::new() };
        let mut out = String::new();
        for id in &ms.buffers {
            if let Some(sb) = self.buffers.get(id) {
                for t in &sb.demux.tracks {
                    let c = match &t.codec {
                        Codec::Av1 { .. } => "av1".to_string(),
                        Codec::Avc { .. } => "avc".to_string(),
                        Codec::Aac { channels, sample_rate, .. } => format!("aac {channels}ch {sample_rate}Hz"),
                        Codec::Other(s) => s.clone(),
                    };
                    let n = sb.samples.get(&t.id).map_or(0, |l| l.lock().unwrap().len());
                    out.push_str(&format!("[{} {c} {}x{} {n} amostras {:?}] ", sb.mime, t.width, t.height, sb.buffered()));
                }
            }
        }
        out
    }
}
