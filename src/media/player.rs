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
