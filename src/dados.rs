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

