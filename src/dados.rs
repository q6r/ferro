//! Dados do perfil em disco: cookies e localStorage sobrevivem ao fechar o
//! navegador (o login continua valendo).
//!
//! Formato: texto, uma entrada por linha, campos separados por tabulação
//! (com `\t`, `\n` e `\\` escapados). Os arquivos ficam na pasta do perfil
//! (`%APPDATA%\Ferro`). Uma thread grava a cada poucos segundos o que mudou;
//! [`flush`] grava na hora (ao fechar a janela).

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static COOKIES_DIRTY: AtomicBool = AtomicBool::new(false);
static STORAGE_DIRTY: AtomicBool = AtomicBool::new(false);

const COOKIES_FILE: &str = "cookies.txt";
const STORAGE_FILE: &str = "armazenamento.txt";

/// Pasta do perfil em uso (None se a persistência está desligada).
pub fn dir() -> Option<PathBuf> {
    DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Pasta padrão do perfil.
pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("Ferro"))
}

/// Liga a persistência: carrega o que está em `dir` e passa a gravar lá.
pub fn enable(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(text) = std::fs::read_to_string(dir.join(COOKIES_FILE)) {
        crate::cookies::import(&text);
    }
    if let Ok(text) = std::fs::read_to_string(dir.join(STORAGE_FILE)) {
        crate::script::import_local_storage(&text);
    }
    let first = DIR.lock().unwrap_or_else(|e| e.into_inner()).replace(dir).is_none();
    if first {
        let _ = std::thread::Builder::new().name("ferro-dados".into()).spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(3));
                flush();
            }
        });
    }
}

/// Os cookies mudaram (a thread grava depois).
pub fn cookies_changed() {
    COOKIES_DIRTY.store(true, Ordering::Relaxed);
}

/// O localStorage mudou.
pub fn storage_changed() {
    STORAGE_DIRTY.store(true, Ordering::Relaxed);
}

/// Grava agora o que mudou.
pub fn flush() {
    let Some(dir) = DIR.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
    if COOKIES_DIRTY.swap(false, Ordering::Relaxed) {
        write_atomic(dir.join(COOKIES_FILE), &crate::cookies::export());
    }
    if STORAGE_DIRTY.swap(false, Ordering::Relaxed) {
        write_atomic(dir.join(STORAGE_FILE), &crate::script::export_local_storage());
    }
}

/// Grava num arquivo temporário e troca (um travamento no meio não corrompe).
fn write_atomic(path: PathBuf, text: &str) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

const HISTORY_FILE: &str = "historico.txt";
const BOOKMARKS_FILE: &str = "favoritos.txt";

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Guarda uma visita no histórico (uma linha: quando, endereço, título).
pub fn add_history(url: &str, title: &str) {
    let Some(dir) = dir() else { return };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(HISTORY_FILE)) {
        let _ = writeln!(f, "{}\t{}\t{}", now_secs(), escape(url), escape(title));
    }
}

/// Histórico, do mais recente para o mais antigo: (quando, endereço, título).
pub fn read_history() -> Vec<(u64, String, String)> {
    let Some(dir) = dir() else { return Vec::new() };
    let text = std::fs::read_to_string(dir.join(HISTORY_FILE)).unwrap_or_default();
    let mut out: Vec<(u64, String, String)> = text
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            Some((f.next()?.parse().ok()?, unescape(f.next()?), unescape(f.next().unwrap_or(""))))
        })
        .collect();
    out.reverse();
    out
}

/// Apaga o histórico.
pub fn clear_history() {
    if let Some(dir) = dir() {
        let _ = std::fs::remove_file(dir.join(HISTORY_FILE));
    }
}

/// Favoritos: (endereço, título), na ordem em que foram salvos.
pub fn read_bookmarks() -> Vec<(String, String)> {
    let Some(dir) = dir() else { return Vec::new() };
    let text = std::fs::read_to_string(dir.join(BOOKMARKS_FILE)).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (u, t) = l.split_once('\t').unwrap_or((l, ""));
            (!u.is_empty()).then(|| (unescape(u), unescape(t)))
        })
        .collect()
}

pub fn is_bookmarked(url: &str) -> bool {
    read_bookmarks().iter().any(|(u, _)| u == url)
}

/// Põe ou tira dos favoritos. Devolve se ficou salvo.
pub fn toggle_bookmark(url: &str, title: &str) -> bool {
    let Some(dir) = dir() else { return false };
    let mut list = read_bookmarks();
    let had = list.iter().any(|(u, _)| u == url);
    if had {
        list.retain(|(u, _)| u != url);
    } else {
        list.push((url.to_string(), title.to_string()));
    }
    let text: String = list.iter().map(|(u, t)| format!("{}\t{}\n", escape(u), escape(t))).collect();
    write_atomic(dir.join(BOOKMARKS_FILE), &text);
    !had
}

/// Escapa um campo (tabulação, quebra de linha e barra invertida).
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        let s = "a\tb\nc\\d\re";
        assert_eq!(unescape(&escape(s)), s);
        assert!(!escape(s).contains('\t'));
    }
}
