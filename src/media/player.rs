//! Reprodução. As amostras de cada trilha ficam numa lista compartilhada com
//! a aba (que só acrescenta e remove o que o site manda por MSE); threads
//! próprias decodificam um pouco à frente do relógio, outra apresenta os
//! quadros na hora certa e o áudio vai para a placa de som pelo `cpal`.
//! Assim o vídeo não para quando o JavaScript da página ocupa a aba.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::decode::{AudioDecoder, Frame, VideoDecoder};
use super::mp4::{Codec, Sample};
use crate::paint::ImageData;

/// Amostras de uma trilha, em ordem de decodificação.
pub type SharedSamples = Arc<Mutex<Vec<Sample>>>;

/// Para onde vão os quadros prontos (a interface): (nó do `<video>`, imagem).
pub type FrameSink = Arc<dyn Fn(usize, Arc<ImageData>) + Send + Sync>;

/// Quanto decodificar à frente do relógio.
const AHEAD: f64 = 2.0;

/// Relógio da mídia, compartilhado com as threads.
#[derive(Clone, Copy)]
pub struct Clock {
    base: f64,
    since: Instant,
    rate: f64,
    running: bool,
}

impl Clock {
    fn now(&self) -> f64 {
        if self.running { self.base + self.since.elapsed().as_secs_f64() * self.rate } else { self.base }
    }
}

type SharedClock = Arc<Mutex<Clock>>;

enum Cmd {
    /// Recomeça a partir deste tempo (busca).
    Seek(f64),
}

/// Fila de quadros decodificados, em ordem de apresentação.
type Queue = Arc<Mutex<VecDeque<Frame>>>;
type Shown = Arc<Mutex<Option<(f64, Arc<ImageData>)>>>;

fn log_on() -> bool {
    std::env::var_os("FERRO_MIDIA_LOG").is_some()
}

/// O dts do quadro-chave em ou antes de `t` (onde começar a decodificar).
fn key_before(samples: &[Sample], t: f64) -> f64 {
    samples
        .iter()
        .rev()
        .find(|s| s.key && s.pts <= t + 0.001)
        .or_else(|| samples.iter().find(|s| s.key))
        .map_or(f64::NEG_INFINITY, |s| s.dts)
}

struct VideoPipe {
    tx: mpsc::Sender<Cmd>,
    shown: Shown,
    /// Quantos quadros já foram apresentados.
    seq: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

impl Drop for VideoPipe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl VideoPipe {
    fn start(codec: &Codec, samples: SharedSamples, clock: SharedClock, node: usize, sink: Option<FrameSink>) -> Option<VideoPipe> {
        let mut dec = VideoDecoder::new(codec)?;
        let (tx, rx) = mpsc::channel::<Cmd>();
        let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
        let shown: Shown = Arc::new(Mutex::new(None));
        let seq = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        // apresentação: escolhe o quadro do momento pelo relógio
        {
            let (q, shown, seq, stop, clock) = (queue.clone(), shown.clone(), seq.clone(), stop.clone(), clock.clone());
            std::thread::Builder::new()
                .name("ferro-video-tela".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(4));
                        let t = clock.lock().unwrap().now();
                        let pick = {
                            let mut g = q.lock().unwrap();
                            let mut pick = None;
                            while g.front().is_some_and(|f| f.pts <= t + 0.008) {
                                pick = g.pop_front();
                            }
                            // nada no tempo ainda (logo depois de uma busca): mostra o primeiro
                            if pick.is_none() && shown.lock().unwrap().is_none() {
                                pick = g.pop_front();
                            }
                            pick
                        };
                        if let Some(f) = pick {
                            let img = f.to_image();
                            *shown.lock().unwrap() = Some((f.pts, img.clone()));
                            seq.fetch_add(1, Ordering::Relaxed);
                            if let Some(sink) = &sink {
                                sink(node, img);
                            }
                        }
                    }
                })
                .ok()?;
        }

        // decodificação: puxa as amostras até AHEAD segundos à frente
        {
            let (q, stop, shown) = (queue.clone(), stop.clone(), shown.clone());
            std::thread::Builder::new()
                .name("ferro-video".into())
                .spawn(move || {
                    let log = log_on();
                    let t0 = clock.lock().unwrap().now();
                    let mut next = key_before(&samples.lock().unwrap(), t0);
                    while !stop.load(Ordering::Relaxed) {
                        while let Ok(Cmd::Seek(t)) = rx.try_recv() {
                            dec.reset();
                            q.lock().unwrap().clear();
                            *shown.lock().unwrap() = None;
                            next = key_before(&samples.lock().unwrap(), t);
                        }
                        let t = clock.lock().unwrap().now();
                        // fila cheia (pausado ou à frente): espera
                        if q.lock().unwrap().len() > 150 {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        let batch: Vec<Sample> = {
                            let list = samples.lock().unwrap();
                            let start = list.partition_point(|s| s.dts < next);
                            list[start..].iter().take_while(|s| s.dts <= t + AHEAD).take(8).cloned().collect()
                        };
                        if batch.is_empty() {
                            std::thread::sleep(Duration::from_millis(8));
                            continue;
                        }
                        for s in batch {
                            next = s.dts + 1e-6;
                            match dec.decode(&s) {
                                Ok(frames) => {
                                    let mut g = q.lock().unwrap();
                                    for f in frames {
                                        // quadro já atrasado não entra (pulo para alcançar o relógio)
                                        if f.pts + 0.05 < t && !g.is_empty() {
                                            continue;
                                        }
                                        let pos = g.partition_point(|x| x.pts <= f.pts);
                                        g.insert(pos, f);
                                    }
                                }
                                Err(e) => {
                                    if log {
                                        eprintln!("[mídia] {e}");
                                    }
                                }
                            }
                        }
                    }
                })
                .ok()?;
        }
        Some(VideoPipe { tx, shown, seq, stop })
    }
}

/// Áudio compartilhado com a callback da placa de som.
struct AudioShared {
    /// PCM intercalado na taxa e canais da trilha.
    pcm: Mutex<VecDeque<f32>>,
    playing: AtomicBool,
    /// Volume (f32 em bits).
    volume: AtomicU32,
    rate: AtomicU32,
    channels: AtomicU32,
}

struct AudioPipe {
    tx: mpsc::Sender<Cmd>,
    shared: Arc<AudioShared>,
    stop: Arc<AtomicBool>,
    _stream: Option<cpal::Stream>,
}

impl Drop for AudioPipe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl AudioPipe {
