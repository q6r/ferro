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

fn get_entry(url: &str) -> Option<Entry> {
    {
        let g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g.as_ref().and_then(|c| c.map.get(url)) {
            return Some(e.clone());
        }
    }
    // disco
    let path = disk_path(url)?;
    let data = std::fs::read(path).ok()?;
    let e = decode_disk(url, &data)?;
    insert_mem(url, e.clone());
    Some(e)
}

/// Procura o endereço no cache.
pub fn lookup(url: &Url) -> Lookup {
    let key = url.as_str();
    let Some(e) = get_entry(key) else { return Lookup::Miss };
    if e.fresh_until > now() {
        return Lookup::Fresh(e.response(url));
    }
    let mut cond = Vec::new();
    if let Some(tag) = e.header("etag") {
        cond.push(("If-None-Match".to_string(), tag.to_string()));
    }
    if let Some(lm) = e.header("last-modified") {
        cond.push(("If-Modified-Since".to_string(), lm.to_string()));
    }
    if cond.is_empty() { Lookup::Miss } else { Lookup::Stale(cond) }
}

/// O servidor respondeu 304: a entrada vale de novo. Devolve a resposta guardada.
pub fn revalidated(url: &Url, headers: &[(String, String)]) -> Option<Response> {
    let key = url.as_str();
    let mut e = get_entry(key)?;
    e.fresh_until = freshness(headers, &e.headers, e.header("content-type").unwrap_or("")).unwrap_or(0);
    let r = e.response(url);
    insert_mem(key, e.clone());
    write_disk(key, &e);
    Some(r)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

