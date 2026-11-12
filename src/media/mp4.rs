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

impl Demuxer {
    pub fn new() -> Demuxer {
        Demuxer::default()
    }

    pub fn track(&self, id: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Acrescenta bytes; devolve as amostras completas encontradas.
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Sample>, String> {
        self.pending.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            let b = &self.pending;
            if b.len() < 8 {
                break;
            }
            let size32 = be32(b, 0) as usize;
            let ty: [u8; 4] = b[4..8].try_into().unwrap();
            let size = match size32 {
                1 => {
                    if b.len() < 16 {
                        break;
                    }
                    be64(b, 8) as usize
                }
                0 => return Err("caixa MP4 de tamanho indefinido".into()),
                s => s,
            };
            if size < 8 {
                return Err(format!("caixa MP4 inválida ({size} bytes)"));
            }
            if b.len() < size {
                break;
            }
            let whole: Vec<u8> = self.pending.drain(..size).collect();
            let hdr = if size32 == 1 { 16 } else { 8 };
            let body = &whole[hdr..];
            match &ty {
                b"moov" => self.read_moov(body)?,
                b"moof" => self.moof = Some(whole.clone()),
                b"mdat" => {
                    if let Some(moof) = self.moof.take() {
                        out.extend(self.read_fragment(&moof, &whole)?);
                    }
                }
                // ftyp, styp, sidx, emsg, free...: nada a fazer
                _ => {}
            }
        }
        Ok(out)
    }

    fn read_moov(&mut self, moov: &[u8]) -> Result<(), String> {
        let mut tracks = Vec::new();
        for (ty, trak) in boxes(moov) {
            if &ty != b"trak" {
                continue;
            }
            let tkhd = child(trak, b"tkhd").ok_or("trak sem tkhd")?;
            let v = tkhd.first().copied().unwrap_or(0);
            let id = if v == 1 { be32(tkhd, 20) } else { be32(tkhd, 12) };
            let mdia = child(trak, b"mdia").ok_or("trak sem mdia")?;
            let mdhd = child(mdia, b"mdhd").ok_or("mdia sem mdhd")?;
            let timescale = if mdhd.first() == Some(&1) { be32(mdhd, 20) } else { be32(mdhd, 12) };
            let stsd = child(mdia, b"minf")
                .and_then(|m| child(m, b"stbl"))
                .and_then(|s| child(s, b"stsd"))
                .ok_or("trak sem stsd")?;
            // stsd: versão/flags (4) + quantidade (4) + entradas
            let entries = boxes(stsd.get(8..).unwrap_or(&[]));
            let Some((fmt, entry)) = entries.first() else { continue };
            let (codec, width, height) = match fmt {
                b"av01" | b"avc1" | b"avc3" | b"encv" => {
                    let width = be16(entry, 24) as u32;
                    let height = be16(entry, 26) as u32;
                    let kids = entry.get(78..).unwrap_or(&[]);
                    let codec = if fmt == b"av01" {
                        let c = child(kids, b"av1C").ok_or("av01 sem av1C")?;
                        Codec::Av1 { config: c.get(4..).unwrap_or(&[]).to_vec() }
                    } else if let Some(c) = child(kids, b"avcC") {
                        Codec::Avc { config: c.to_vec() }
                    } else {
                        Codec::Other(String::from_utf8_lossy(fmt).into_owned())
                    };
                    (codec, width, height)
                }
                b"mp4a" => {
                    let channels = be16(entry, 16);
                    let sample_rate = be32(entry, 24) >> 16;
                    let kids = entry.get(28..).unwrap_or(&[]);
                    let config = child(kids, b"esds").and_then(esds_config).unwrap_or_default();
                    (Codec::Aac { config, channels, sample_rate }, 0, 0)
                }
                other => (Codec::Other(String::from_utf8_lossy(other).into_owned()), 0, 0),
            };
            tracks.push(Track {
                id,
                timescale: timescale.max(1),
                codec,
                width,
                height,
                default_duration: 0,
                default_size: 0,
                default_flags: 0,
            });
        }
        // padrões dos fragmentos (mvex/trex)
        if let Some(mvex) = child(moov, b"mvex") {
            for (ty, trex) in boxes(mvex) {
                if &ty != b"trex" {
                    continue;
                }
                let id = be32(trex, 4);
                if let Some(t) = tracks.iter_mut().find(|t| t.id == id) {
                    t.default_duration = be32(trex, 12);
                    t.default_size = be32(trex, 16);
                    t.default_flags = be32(trex, 20);
                }
            }
        }
        if tracks.is_empty() {
            return Err("moov sem trilhas".into());
        }
        // um novo segmento de inicialização substitui o anterior (changeType, troca de qualidade)
        self.tracks = tracks;
        Ok(())
    }

    /// Amostras de um `moof` seguido do seu `mdat` (ambos com cabeçalho).
    fn read_fragment(&self, moof: &[u8], mdat: &[u8]) -> Result<Vec<Sample>, String> {
        let mut out = Vec::new();
        let moof_body = &moof[8..];
        // o mdat vem logo depois do moof no fluxo: posições relativas ao início do moof
        let stream: Vec<u8> = [moof, mdat].concat();
        for (ty, traf) in boxes(moof_body) {
            if &ty != b"traf" {
                continue;
            }
            let tfhd = child(traf, b"tfhd").ok_or("traf sem tfhd")?;
            let flags = be32(tfhd, 0) & 0xff_ffff;
            let id = be32(tfhd, 4);
            let Some(track) = self.track(id) else { continue };
            let mut p = 8;
            let mut base = 0u64; // relativo ao início do moof
            if flags & 0x1 != 0 {
                // base-data-offset absoluto: raro em MSE; trata como relativo ao moof
                base = 0;
                p += 8;
            }
            if flags & 0x2 != 0 {
                p += 4;
            }
            let mut def_dur = track.default_duration;
            let mut def_size = track.default_size;
            let mut def_flags = track.default_flags;
            if flags & 0x8 != 0 {
                def_dur = be32(tfhd, p);
                p += 4;
            }
            if flags & 0x10 != 0 {
                def_size = be32(tfhd, p);
                p += 4;
            }
            if flags & 0x20 != 0 {
                def_flags = be32(tfhd, p);
            }
            let mut decode_time = match child(traf, b"tfdt") {
                Some(t) if t.first() == Some(&1) => be64(t, 4),
                Some(t) => be32(t, 4) as u64,
                None => 0,
            };
            let ts = track.timescale as f64;
            for (rty, trun) in boxes(traf) {
                if &rty != b"trun" {
                    continue;
                }
                let version = trun.first().copied().unwrap_or(0);
                let tf = be32(trun, 0) & 0xff_ffff;
                let count = be32(trun, 4) as usize;
                let mut q = 8;
                let mut offset = base;
                if tf & 0x1 != 0 {
                    offset = (base as i64 + be32(trun, q) as i32 as i64) as u64;
                    q += 4;
                }
                let mut first_flags = None;
                if tf & 0x4 != 0 {
                    first_flags = Some(be32(trun, q));
                    q += 4;
                }
                for k in 0..count.min(100_000) {
                    let dur = if tf & 0x100 != 0 {
                        q += 4;
                        be32(trun, q - 4)
                    } else {
                        def_dur
                    };
                    let size = if tf & 0x200 != 0 {
                        q += 4;
                        be32(trun, q - 4)
                    } else {
                        def_size
                    };
                    let sflags = if tf & 0x400 != 0 {
                        q += 4;
                        be32(trun, q - 4)
                    } else if k == 0 {
                        first_flags.unwrap_or(def_flags)
                    } else {
                        def_flags
                    };
                    let cto: i64 = if tf & 0x800 != 0 {
                        q += 4;
                        let raw = be32(trun, q - 4);
                        if version == 1 { raw as i32 as i64 } else { raw as i64 }
                    } else {
                        0
                    };
                    let start = offset as usize;
                    let end = start + size as usize;
                    let Some(data) = stream.get(start..end) else {
                        return Err("amostra fora do mdat".into());
                    };
                    // sample_is_non_sync_sample = bit 16
                    let key = if track.is_video() { sflags & 0x0001_0000 == 0 } else { true };
                    out.push(Sample {
                        track: id,
                        dts: decode_time as f64 / ts,
                        pts: (decode_time as i64 + cto) as f64 / ts,
                        duration: dur as f64 / ts,
                        key,
                        data: Arc::from(data),
                    });
                    decode_time += dur as u64;
                    offset += size as u64;
                }
            }
        }
        Ok(out)
    }
}

