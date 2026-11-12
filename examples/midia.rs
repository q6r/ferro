//! Testa o leitor de MP4 fragmentado e os decodificadores com arquivos
//! gravados por FERRO_MIDIA_DUMP: `cargo run --release --example midia -- sb2.mp4 [pasta-png]`.

#[global_allocator]
static ALLOC: ferro::alloc::Alloc = ferro::alloc::Alloc;

use ferro::media::decode::{AudioDecoder, VideoDecoder};
use ferro::media::mp4::{Codec, Demuxer};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("arquivo");
    let out = args.get(1).cloned();
    let mut d = Demuxer::new();
    let mut samples = Vec::new();
    for chunk in data.chunks(64 * 1024) {
        samples.extend(d.push(chunk).expect("mp4"));
    }
    for t in &d.tracks {
        let c = match &t.codec {
            Codec::Av1 { config } => format!("AV1 (config {} bytes)", config.len()),
            Codec::Avc { config } => format!("H.264 (avcC {} bytes)", config.len()),
            Codec::Aac { channels, sample_rate, config } => format!("AAC {channels}ch {sample_rate}Hz asc={config:02x?}"),
            Codec::Other(s) => s.clone(),
        };
        println!("trilha {} timescale {} {}x{} {c}", t.id, t.timescale, t.width, t.height);
    }
    println!("{} amostras, {:.2}s a {:.2}s, {} chave", samples.len(), samples.first().map_or(0.0, |s| s.pts), samples.last().map_or(0.0, |s| s.pts), samples.iter().filter(|s| s.key).count());
    let track = &d.tracks[0];
    let t = std::time::Instant::now();
    if track.is_video() {
        let mut dec = VideoDecoder::new(&track.codec).expect("decodificador");
        let mut n = 0;
        for s in samples.iter().take(240) {
            match dec.decode(s) {
                Ok(frames) => {
                    for f in frames {
                        n += 1;
                        if let Some(dir) = &out && n % 60 == 1 {
                            let im = f.to_image();
                            let img = image::RgbaImage::from_raw(im.width, im.height, im.rgba.clone()).unwrap();
                            img.save(format!("{dir}/quadro{n:04}.png")).unwrap();
                        }
                    }
                }
                Err(e) => { println!("erro: {e}"); break; }
            }
        }
        let el = t.elapsed().as_secs_f64();
        println!("{n} quadros em {:.0} ms ({:.1} quadros/s)", el * 1000.0, n as f64 / el);
    } else {
        let mut dec = AudioDecoder::new(&track.codec).expect("decodificador");
        let mut total = 0usize;
        for s in &samples {
            match dec.decode(s) {
                Ok(pcm) => total += pcm.len(),
                Err(e) => { println!("erro: {e}"); break; }
            }
        }
        println!("{} amostras PCM = {:.2}s de áudio ({} Hz, {} canais), em {:.0} ms", total, total as f64 / dec.channels as f64 / dec.sample_rate as f64, dec.sample_rate, dec.channels, t.elapsed().as_secs_f64() * 1000.0);
    }
}
