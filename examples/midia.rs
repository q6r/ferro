//! Teste de mídia: lê um arquivo, decodifica as trilhas e mede.
//!
//! Serve para MP4 fragmentado e WebM (inclusive os gravados por
//! `FERRO_MIDIA_DUMP`, lidos em pedaços como no MSE) e para MP4 comum.
//!
//! `cargo run --release --example midia -- arquivo [segundos | pasta-png]`
//! - um número limita a decodificação aos primeiros segundos;
//! - uma pasta recebe um PNG a cada 60 quadros;
//! - `MIDIA_PNG=quadro.png` salva o último quadro decodificado.

#[global_allocator]
static ALLOC: ferro::alloc::Alloc = ferro::alloc::Alloc;

use std::time::Instant;

use ferro::media::decode::{AudioDecoder, VideoDecoder};
use ferro::media::mp4::{self, Codec, Demuxer, Sample, Track};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("uso: midia <arquivo> [segundos | pasta-png]");
        std::process::exit(2);
    };
    let (limit, png_dir) = match args.get(1) {
        Some(a) => match a.parse::<f64>() {
            Ok(s) => (s, None),
            Err(_) => (f64::INFINITY, Some(a.clone())),
        },
        None => (f64::INFINITY, None),
    };
    let data = std::fs::read(path).expect("ler arquivo");
    let t0 = Instant::now();
    let (tracks, samples) = read(&data);
    println!(
        "contêiner: {} trilhas, {} amostras ({} quadros-chave) em {:?}",
        tracks.len(),
        samples.len(),
        samples.iter().filter(|s| s.key).count(),
        t0.elapsed()
    );
    for t in &tracks {
        let mine: Vec<&Sample> = samples.iter().filter(|s| s.track == t.id).collect();
        let first = mine.first().map_or(0.0, |s| s.pts);
        let last = mine.iter().map(|s| s.pts + s.duration).fold(0.0, f64::max);
        println!(
            "  trilha {}: {} {}x{} · {} amostras de {first:.2} s a {last:.2} s",
            t.id,
            codec_name(&t.codec),
            t.width,
            t.height,
            mine.len()
        );
    }
    for t in &tracks {
        let list: Vec<&Sample> = samples.iter().filter(|s| s.track == t.id && s.pts < limit).collect();
        if t.is_video() {
            video(t, &list, png_dir.as_deref());
        } else if t.is_audio() {
            audio(t, &list);
        }
    }
}

/// Lê em pedaços de 64 KB, como o MSE (MP4 fragmentado e WebM); se não sair
/// nada, tenta como MP4 comum (tabelas do `moov`).
fn read(data: &[u8]) -> (Vec<Track>, Vec<Sample>) {
    let mut d = Demuxer::new();
    let mut samples = Vec::new();
    let mut ok = true;
    for chunk in data.chunks(64 * 1024) {
        match d.push(chunk) {
            Ok(s) => samples.extend(s),
            Err(e) => {
                println!("leitura em pedaços: {e}");
                ok = false;
                break;
            }
        }
    }
    if ok && !samples.is_empty() {
        return (d.tracks, samples);
    }
    mp4::demux_file(data).expect("ler contêiner")
}

fn video(t: &Track, list: &[&Sample], png_dir: Option<&str>) {
    let Some(mut dec) = VideoDecoder::new(&t.codec) else {
        println!("  vídeo {}: sem decodificador", t.id);
        return;
    };
    let t1 = Instant::now();
    let (mut frames, mut errs, mut size) = (0usize, 0usize, (0, 0));
    for s in list {
        match dec.decode(s) {
            Ok(fs) => {
                for f in fs {
                    frames += 1;
                    size = (f.width, f.height);
                    if let Some(dir) = png_dir
                        && frames % 60 == 1
                    {
                        let _ = std::fs::write(format!("{dir}/quadro{frames:04}.png"), ferro::canvas::encode_png(&f.to_image()));
                    }
                    if let Some(out) = std::env::var_os("MIDIA_PNG") {
                        let _ = std::fs::write(out, ferro::canvas::encode_png(&f.to_image()));
                    }
                }
            }
            Err(e) => {
                if errs < 3 {
                    println!("    erro: {e}");
                }
                errs += 1;
            }
        }
    }
    let el = t1.elapsed().as_secs_f64();
    let media = list.last().map_or(0.0, |s| s.pts + s.duration) - list.first().map_or(0.0, |s| s.pts);
    println!(
        "  vídeo: {frames} quadros {}x{} em {el:.2} s ({:.0} quadros/s; {:.1}x tempo real), {errs} erros",
        size.0,
        size.1,
        frames as f64 / el.max(1e-9),
        media / el.max(1e-9)
    );
}

fn audio(t: &Track, list: &[&Sample]) {
    let Some(mut dec) = AudioDecoder::new(&t.codec) else {
        println!("  áudio {}: sem decodificador", t.id);
        return;
    };
    let t1 = Instant::now();
    let (mut pcm, mut errs, mut peak) = (0usize, 0usize, 0f32);
    for s in list {
        match dec.decode(s) {
            Ok(p) => {
                peak = p.iter().fold(peak, |m, x| m.max(x.abs()));
                pcm += p.len();
            }
            Err(e) => {
                if errs < 3 {
                    println!("    erro: {e}");
                }
                errs += 1;
            }
        }
    }
    let secs = pcm as f64 / (dec.sample_rate as f64 * dec.channels as f64);
    println!(
        "  áudio: {secs:.2} s de som ({} Hz, {} canais, pico {peak:.2}) em {:.3} s, {errs} erros",
        dec.sample_rate,
        dec.channels,
        t1.elapsed().as_secs_f64()
    );
}

fn codec_name(c: &Codec) -> String {
    match c {
        Codec::Av1 { config } => format!("AV1 (config {} bytes)", config.len()),
        Codec::Avc { config } => format!("H.264 (avcC {} bytes)", config.len()),
        Codec::Aac { channels, sample_rate, .. } => format!("AAC {channels}ch {sample_rate}Hz"),
        Codec::Vp9 => "VP9".into(),
        Codec::Vp8 => "VP8".into(),
        Codec::Opus { .. } => "Opus".into(),
        Codec::Vorbis { .. } => "Vorbis".into(),
        Codec::Other(s) => s.clone(),
    }
}
