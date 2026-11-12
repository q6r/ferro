//! Cache HTTP: guarda as respostas que o servidor deixa guardar
//! (`Cache-Control: max-age`, `Expires`, `ETag`/`Last-Modified`) na memória
//! e, com o perfil ligado, em disco. Voltar a um site (ou abrir outra aba
//! nele) não baixa de novo os scripts e imagens.
//!
//! Regras (um cache privado de navegador, simplificado):
//! - só GET, status 200, sem `Range`, e sem redirecionamento no caminho;
//! - `no-store` não guarda; `no-cache` guarda mas sempre revalida;
//! - documentos HTML só com validade explícita (nada de heurística);
//! - vencida com validador: pergunta ao servidor (`If-None-Match` /
//!   `If-Modified-Since`) e o 304 renova a entrada.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use url::Url;

use crate::net::Response;

/// Memória máxima do cache (o disco guarda mais).
const MEM_LIMIT: usize = 256 * 1024 * 1024;
/// Respostas maiores que isso não entram.
const MAX_ENTRY: usize = 40 * 1024 * 1024;

#[derive(Clone)]
struct Entry {
    status: u16,
    headers: Vec<(String, String)>,
    body: Arc<Vec<u8>>,
    /// Segundos (UNIX) até quando vale sem perguntar ao servidor.
    fresh_until: u64,
}

impl Entry {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn response(&self, url: &Url) -> Response {
        Response {
            url: url.clone(),
            status: self.status,
            content_type: self.header("content-type").map(str::to_string),
            body: self.body.as_ref().clone(),
            headers: self.headers.clone(),
        }
    }
}

#[derive(Default)]
struct Cache {
    map: HashMap<String, Entry>,
    order: VecDeque<String>,
    bytes: usize,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Pasta do cache em disco (dentro do perfil), se o perfil estiver ligado.
fn disk_dir() -> Option<PathBuf> {
    crate::dados::dir().map(|d| d.join("cache"))
}

fn disk_path(url: &str) -> Option<PathBuf> {
    // FNV-1a de 64 bits do endereço
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(disk_dir()?.join(format!("{h:016x}")))
}

/// O que a consulta achou.
pub enum Lookup {
    /// Pode usar sem perguntar.
    Fresh(Response),
    /// Vencida: estes cabeçalhos perguntam se ainda vale.
    Stale(Vec<(String, String)>),
    Miss,
}

