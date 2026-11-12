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

/// Data dos cookies (`Expires=`): aceita os formatos que os servidores usam
/// ("Wed, 21 Oct 2015 07:28:00 GMT", "Wednesday, 21-Oct-15 07:28:00 GMT"...).
fn parse_date(s: &str) -> Option<f64> {
    let (mut day, mut month, mut year, mut time) = (None, None, None, None);
    for tok in s.split(|c: char| !c.is_ascii_alphanumeric() && c != ':').filter(|t| !t.is_empty()) {
        if time.is_none() && tok.contains(':') {
            let p: Vec<u32> = tok.split(':').filter_map(|x| x.parse().ok()).collect();
            if p.len() >= 2 {
                time = Some((p[0], p[1], p.get(2).copied().unwrap_or(0)));
                continue;
            }
        }
        if month.is_none() && tok.len() >= 3 {
            let l = tok[..3].to_ascii_lowercase();
            if let Some(m) = MONTHS.iter().position(|&m| m == l) {
                month = Some(m as u32 + 1);
                continue;
            }
        }
        if let Ok(n) = tok.parse::<u32>() {
            if day.is_none() && (1..=31).contains(&n) && tok.len() <= 2 {
                day = Some(n);
            } else if year.is_none() {
                year = Some(match n {
                    0..=69 => n + 2000,
                    70..=99 => n + 1900,
                    _ => n,
                });
            }
        }
    }
    let (d, m, y) = (day?, month?, year?);
    let (hh, mm, ss) = time.unwrap_or((0, 0, 0));
    // dias desde 1970 (algoritmo de Howard Hinnant)
    let (y, m) = (y as i64 - if m <= 2 { 1 } else { 0 }, m as i64);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + hh as i64 * 3600 + mm as i64 * 60 + ss as i64) as f64)
}

/// Data HTTP (`Expires`, `Last-Modified`) em segundos desde 1970.
pub fn parse_http_date(s: &str) -> Option<f64> {
    parse_date(s)
}

