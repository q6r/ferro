//! CORS: o que uma página pode pedir e ler de outros sites (fetch e
//! XMLHttpRequest), seguindo o padrão Fetch como o Chrome:
//!
//! - pedido para outro site leva `Origin`; cookies só com `credentials: 'include'`
//!   (ou `withCredentials`);
//! - pedido que não é "simples" (método ou cabeçalhos fora da lista segura)
//!   passa antes por um OPTIONS (preflight), guardado num cache curto;
//! - a resposta só chega à página se o site deixar (`Access-Control-Allow-Origin`),
//!   e só com os cabeçalhos liberados;
//! - `mode: 'no-cors'` devolve uma resposta opaca (sem corpo nem cabeçalhos).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use url::Url;

use crate::net::{Net, Request, Response};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Cors,
    NoCors,
    SameOrigin,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "no-cors" => Mode::NoCors,
            "same-origin" => Mode::SameOrigin,
            _ => Mode::Cors,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Credentials {
    Omit,
    SameOrigin,
    Include,
}

impl Credentials {
    pub fn parse(s: &str) -> Credentials {
        match s {
            "omit" => Credentials::Omit,
            "include" => Credentials::Include,
            _ => Credentials::SameOrigin,
        }
    }
}

/// Tipo da resposta, como em `Response.type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Basic,
    Cors,
    Opaque,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Basic => "basic",
            Kind::Cors => "cors",
            Kind::Opaque => "opaque",
        }
    }
}

/// Um pedido feito por uma página.
pub struct PageRequest {
    pub req: Request,
    /// Origem da página, serializada ("https://a.com"; "null" se opaca).
    pub origin: String,
    pub mode: Mode,
    pub credentials: Credentials,
    /// Nomes dos cabeçalhos postos pela página (os do navegador não contam
    /// para o preflight).
    pub author_headers: Vec<String>,
    /// "fetch" ou "XMLHttpRequest" (para a mensagem de erro).
    pub api: &'static str,
}

/// O endereço é de outro site, visto da página em `origin`? Só http(s) entra
/// no CORS; `data:` vale como mesma origem, e `file:`/`ferro:` têm regra
/// própria (`net::allowed`).
pub fn is_cross(origin: &str, url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => url.origin().ascii_serialization() != origin,
        _ => false,
    }
}

/// Cabeçalhos que a página não pode definir (o navegador cuida deles).
pub fn forbidden_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
            | "connection"
            | "content-length"
            | "cookie"
            | "cookie2"
            | "date"
            | "dnt"
            | "expect"
            | "host"
            | "keep-alive"
            | "origin"
            | "referer"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "via"
    ) || n.starts_with("proxy-")
        || n.starts_with("sec-")
}

/// Cabeçalho de pedido na lista segura (não exige preflight).
fn safelisted(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    let unsafe_byte = |c: char| {
        (c < ' ' && c != '\t') || matches!(c, '"' | '(' | ')' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '{' | '}' | '\x7f')
    };
    match name.to_ascii_lowercase().as_str() {
        "accept" => !value.chars().any(unsafe_byte),
        "accept-language" | "content-language" => value.chars().all(|c| c.is_ascii_alphanumeric() || " *,-.;=".contains(c)),
        "content-type" => {
            if value.chars().any(unsafe_byte) {
                return false;
            }
            let essence = value.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            matches!(essence.as_str(), "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain")
        }
        "range" => {
            // só "bytes=N-" ou "bytes=N-M"
            let Some(r) = value.strip_prefix("bytes=") else { return false };
            let Some((a, b)) = r.split_once('-') else { return false };
            !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

fn header<'a>(res: &'a Response, name: &str) -> Option<&'a str> {
    res.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.trim())
}

fn list(v: Option<&str>) -> Vec<String> {
    v.unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// A resposta libera a página? Devolve o motivo do bloqueio, como o Chrome.
fn check(res: &Response, origin: &str, include: bool) -> Result<(), String> {
    match header(res, "access-control-allow-origin") {
        None => Err("No 'Access-Control-Allow-Origin' header is present on the requested resource.".into()),
        Some("*") if !include => Ok(()),
        Some("*") => Err("The value of the 'Access-Control-Allow-Origin' header in the response must not be the wildcard '*' \
                          when the request's credentials mode is 'include'."
            .into()),
        Some(v) if v == origin => {
            if include && header(res, "access-control-allow-credentials") != Some("true") {
                Err("The value of the 'Access-Control-Allow-Credentials' header in the response is '' which must be 'true' \
                     when the request's credentials mode is 'include'."
                    .into())
            } else {
                Ok(())
            }
        }
        Some(v) => Err(format!("The 'Access-Control-Allow-Origin' header has a value '{v}' that is not equal to the supplied origin.")),
    }
}

/// Só os cabeçalhos que a página pode ver numa resposta de outro site.
fn expose(mut res: Response, include: bool) -> Response {
    const SAFE: [&str; 7] = ["cache-control", "content-language", "content-length", "content-type", "expires", "last-modified", "pragma"];
    let extra: Vec<String> = list(header(&res, "access-control-expose-headers")).iter().map(|s| s.to_ascii_lowercase()).collect();
    let all = !include && extra.iter().any(|e| e == "*");
    res.headers.retain(|(k, _)| k != "set-cookie" && (all || SAFE.contains(&k.as_str()) || extra.contains(k)));
    res
}

/// O que um preflight liberou, por (origem, endereço, com credenciais).
struct Allowed {
    until: Instant,
    methods: Vec<String>,
    headers: Vec<String>,
}

/// (origem, endereço, com credenciais) → o que foi liberado.
type PreflightCache = HashMap<(String, String, bool), Allowed>;

static PREFLIGHTS: Mutex<Option<PreflightCache>> = Mutex::new(None);

fn preflight(net: &Net, p: &PageRequest, unsafe_headers: &[String], include: bool) -> Result<(), String> {
    let method = p.req.method.to_ascii_uppercase();
    let key = (p.origin.clone(), p.req.url.to_string(), include);
    let covered = |a: &Allowed| {
        let m_ok = matches!(method.as_str(), "GET" | "HEAD" | "POST")
            || a.methods.iter().any(|m| m.eq_ignore_ascii_case(&method))
            || (!include && a.methods.iter().any(|m| m == "*"));
        let h_ok = unsafe_headers.iter().all(|h| {
            a.headers.iter().any(|x| x.eq_ignore_ascii_case(h)) || (!include && h != "authorization" && a.headers.iter().any(|x| x == "*"))
        });
        m_ok && h_ok
    };
    {
        let mut g = PREFLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
        let map = g.get_or_insert_with(HashMap::new);
        map.retain(|_, a| a.until > Instant::now());
        if map.get(&key).is_some_and(covered) {
            return Ok(());
        }
    }
    let mut req = Request::get(&p.req.url);
    req.method = "OPTIONS".into();
    req.cookies = false;
    req.headers = vec![
        ("Accept".into(), "*/*".into()),
        ("Origin".into(), p.origin.clone()),
        ("Access-Control-Request-Method".into(), method.clone()),
    ];
    if !unsafe_headers.is_empty() {
        req.headers.push(("Access-Control-Request-Headers".into(), unsafe_headers.join(",")));
    }
    let res = net.request(&req)?;
    if !(200..300).contains(&res.status) {
        return Err("Response to preflight request doesn't pass access control check: It does not have HTTP ok status.".into());
    }
    check(&res, &p.origin, include).map_err(|e| format!("Response to preflight request doesn't pass access control check: {e}"))?;
    let age = header(&res, "access-control-max-age").and_then(|v| v.parse::<u64>().ok()).unwrap_or(5).min(7200);
    let allowed = Allowed {
        until: Instant::now() + Duration::from_secs(age),
        methods: list(header(&res, "access-control-allow-methods")),
        headers: list(header(&res, "access-control-allow-headers")),
    };
    if !covered(&allowed) {
        let bad_method = !matches!(method.as_str(), "GET" | "HEAD" | "POST")
            && !allowed.methods.iter().any(|m| m.eq_ignore_ascii_case(&method) || (!include && m == "*"));
        return Err(if bad_method {
            format!("Method {method} is not allowed by Access-Control-Allow-Methods in preflight response.")
        } else {
            let h = unsafe_headers
                .iter()
                .find(|h| !allowed.headers.iter().any(|x| x.eq_ignore_ascii_case(h) || (!include && x == "*" && *h != "authorization")))
                .cloned()
                .unwrap_or_default();
            format!("Request header field {h} is not allowed by Access-Control-Allow-Headers in preflight response.")
        });
    }
    PREFLIGHTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, allowed);
    Ok(())
}

/// Faz o pedido de uma página seguindo o CORS.
pub fn fetch(net: &Net, mut p: PageRequest) -> Result<(Response, Kind), String> {
    let url = p.req.url.clone();
    let blocked = |why: &str, origin: &str| format!("Access to {} at '{url}' from origin '{origin}' has been blocked by CORS policy: {why}", p.api);
    if !is_cross(&p.origin, &url) {
        p.req.cookies = p.credentials != Credentials::Omit;
        let res = net.request(&p.req)?;
        // redirecionou para outro site: a resposta precisa da liberação dele
        if p.mode == Mode::Cors && is_cross(&p.origin, &res.url) {
            let include = p.credentials == Credentials::Include;
            check(&res, &p.origin, include).map_err(|e| blocked(&e, &p.origin))?;
            return Ok((expose(res, include), Kind::Cors));
        }
        return Ok((res, Kind::Basic));
    }
    let method = p.req.method.to_ascii_uppercase();
    let include = p.credentials == Credentials::Include;
    p.req.cookies = include;
    p.req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("origin"));
    p.req.headers.push(("Origin".into(), p.origin.clone()));
    match p.mode {
        Mode::SameOrigin => Err(format!("Failed to fetch: o modo 'same-origin' não permite pedir {url}")),
        Mode::NoCors => {
            if !matches!(method.as_str(), "GET" | "HEAD" | "POST") {
                return Err(format!("Failed to execute 'fetch': '{method}' is unsupported in no-cors mode."));
            }
            let res = net.request(&p.req)?;
            Ok((
                Response {
                    url: res.url,
                    status: 0,
                    content_type: None,
                    body: Vec::new(),
                    headers: Vec::new(),
                },
                Kind::Opaque,
            ))
        }
        Mode::Cors => {
            let mut unsafe_headers: Vec<String> = p
                .author_headers
                .iter()
                .filter(|name| {
                    let v = p.req.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map_or("", |(_, v)| v.as_str());
                    !safelisted(name, v)
                })
                .map(|n| n.to_ascii_lowercase())
                .collect();
            unsafe_headers.sort();
            unsafe_headers.dedup();
            let simple_method = matches!(method.as_str(), "GET" | "HEAD" | "POST");
            if !simple_method || !unsafe_headers.is_empty() {
                preflight(net, &p, &unsafe_headers, include).map_err(|e| blocked(&e, &p.origin))?;
            }
            let res = net.request(&p.req)?;
            check(&res, &p.origin, include).map_err(|e| format!("{} (HTTP {})", blocked(&e, &p.origin), res.status))?;
            Ok((expose(res, include), Kind::Cors))
        }
    }
}

/// `<img crossorigin>`: a imagem de outro site pode ir para um canvas sem
/// sujá-lo? (pede com `Origin` e confere a liberação)
pub fn fetch_image(net: &Net, url: &Url, origin: &str, include: bool) -> Result<Response, String> {
    let mut req = Request::get_accepting(url, crate::net::ACCEPT_IMAGE);
    req.cookies = include;
    req.headers.push(("Origin".into(), origin.to_string()));
    let res = net.request(&req)?;
    check(&res, origin, include)?;
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(headers: &[(&str, &str)]) -> Response {
        Response {
            url: Url::parse("https://api.b.com/x").unwrap(),
            status: 200,
            content_type: None,
            body: b"segredo".to_vec(),
            headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn origem_e_liberacao() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(!is_cross("https://a.com", &u("https://a.com/x")));
        assert!(is_cross("https://a.com", &u("https://b.a.com/x")));
        assert!(is_cross("https://a.com", &u("http://a.com/x")));
        assert!(is_cross("null", &u("https://a.com/x")));
        assert!(!is_cross("https://a.com", &u("data:text/plain,oi")));

        assert!(check(&res(&[]), "https://a.com", false).is_err());
        assert!(check(&res(&[("access-control-allow-origin", "*")]), "https://a.com", false).is_ok());
        assert!(check(&res(&[("access-control-allow-origin", "*")]), "https://a.com", true).is_err());
        assert!(check(&res(&[("access-control-allow-origin", "https://a.com")]), "https://a.com", true).is_err());
        assert!(
            check(
                &res(&[("access-control-allow-origin", "https://a.com"), ("access-control-allow-credentials", "true")]),
                "https://a.com",
                true
            )
            .is_ok()
        );
        assert!(check(&res(&[("access-control-allow-origin", "https://c.com")]), "https://a.com", false).is_err());
    }

    #[test]
    fn cabecalhos_seguros_e_expostos() {
        assert!(safelisted("Content-Type", "text/plain;charset=UTF-8"));
        assert!(!safelisted("Content-Type", "application/json"));
        assert!(safelisted("Accept", "*/*"));
        assert!(safelisted("Range", "bytes=0-99"));
        assert!(!safelisted("Range", "bytes=0-1,5-9"));
        assert!(!safelisted("X-Token", "a"));
        assert!(forbidden_header("Cookie") && forbidden_header("sec-fetch-mode") && !forbidden_header("x-a"));

        let r = expose(
            res(&[
                ("content-type", "text/plain"),
                ("x-segredo", "1"),
                ("x-livre", "2"),
                ("access-control-expose-headers", "X-Livre"),
            ]),
            false,
        );
        let names: Vec<&str> = r.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["content-type", "x-livre"]);
    }
}
