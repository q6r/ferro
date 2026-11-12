//! Cookies (RFC 6265): um pote só para o navegador inteiro, compartilhado por
//! todas as abas. Guarda o que vem nos `Set-Cookie` e no `document.cookie`, e
//! devolve o cabeçalho `Cookie` de cada requisição.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use url::Url;

#[derive(Clone, Debug)]
struct Cookie {
    name: String,
    value: String,
    /// Domínio, sem o ponto inicial.
    domain: String,
    /// Sem `Domain=`: vale só para o host exato.
    host_only: bool,
    path: String,
    secure: bool,
    http_only: bool,
    /// Segundos desde 1970; None = cookie de sessão.
    expires: Option<f64>,
    /// Ordem de criação (desempate na ordenação).
    created: u64,
}

struct Jar {
    cookies: Vec<Cookie>,
    seq: u64,
}

static JAR: Mutex<Jar> = Mutex::new(Jar {
    cookies: Vec::new(),
    seq: 0,
});

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn domain_match(host: &str, domain: &str) -> bool {
    host == domain || (host.ends_with(domain) && host[..host.len() - domain.len()].ends_with('.'))
}

fn path_match(req: &str, path: &str) -> bool {
    req == path
        || (req.starts_with(path) && (path.ends_with('/') || req[path.len()..].starts_with('/')))
}

/// Caminho padrão: o "diretório" do caminho da URL.
fn default_path(url: &Url) -> String {
    let p = url.path();
    match p.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => p[..i].to_string(),
    }
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

