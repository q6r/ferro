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
