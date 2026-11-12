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

