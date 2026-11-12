//! Teste de mídia: lê um arquivo (MP4 ou WebM), decodifica as trilhas e mede.
//!
//! `cargo run --release --example midia -- arquivo.webm [segundos]`

use std::time::Instant;

use ferro::media::decode::{AudioDecoder, VideoDecoder};
use ferro::media::mp4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("uso: midia <arquivo> [segundos]");
        std::process::exit(2);
    };
    let limit: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(f64::INFINITY);
    let data = std::fs::read(path).expect("ler arquivo");
    let t0 = Instant::now();
    let (tracks, samples) = mp4::demux_file(&data).expect("ler contêiner");
    println!("contêiner: {} trilhas, {} amostras em {:?}", tracks.len(), samples.len(), t0.elapsed());
    for t in &tracks {
        let n = samples.iter().filter(|s| s.track == t.id).count();
        let last = samples.iter().filter(|s| s.track == t.id).map(|s| s.pts + s.duration).fold(0.0, f64::max);
        println!("  trilha {}: {:?} {}x{} · {n} amostras até {last:.2} s", t.id, codec_name(&t.codec), t.width, t.height);
    }
    for t in &tracks {
        let list: Vec<_> = samples.iter().filter(|s| s.track == t.id && s.pts < limit).collect();
        if t.is_video() {
            let Some(mut dec) = VideoDecoder::new(&t.codec) else {
                println!("  vídeo {}: sem decodificador", t.id);
                continue;
            };
            let t1 = Instant::now();
            let (mut frames, mut errs, mut size) = (0usize, 0usize, (0, 0));
            for s in &list {
                match dec.decode(s) {
                    Ok(fs) => {
                        if let Some(f) = fs.last() {
                            size = (f.width, f.height);
                            // MIDIA_PNG=arquivo.png: salva o último quadro
                            if let Some(out) = std::env::var_os("MIDIA_PNG") {
                                let _ = std::fs::write(out, ferro::canvas::encode_png(&f.to_image()));
                            }
                        }
                        frames += fs.len();
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
        } else if t.is_audio() {
            let Some(mut dec) = AudioDecoder::new(&t.codec) else {
                println!("  áudio {}: sem decodificador", t.id);
                continue;
            };
            let t1 = Instant::now();
            let (mut pcm, mut errs, mut peak) = (0usize, 0usize, 0f32);
            for s in &list {
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
    }
}

fn codec_name(c: &mp4::Codec) -> String {
    match c {
        mp4::Codec::Av1 { .. } => "AV1".into(),
        mp4::Codec::Avc { .. } => "H.264".into(),
        mp4::Codec::Aac { .. } => "AAC".into(),
        mp4::Codec::Vp9 => "VP9".into(),
        mp4::Codec::Vp8 => "VP8".into(),
        mp4::Codec::Vorbis { .. } => "Vorbis".into(),
        mp4::Codec::Opus { .. } => "Opus".into(),
        mp4::Codec::Other(s) => s.clone(),
    }
}
