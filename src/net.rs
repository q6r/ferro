//! Rede: busca recursos por http(s), file: e data:, e decodifica o texto
//! (UTF-8 ou Windows-1252/Latin-1, comum em sites brasileiros antigos).
//!
//! TLS: rustls com criptografia do RustCrypto (100% Rust, sem C nem assembly).

use std::sync::Arc;
use std::time::Duration;

use url::Url;

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Ferro/0.1";

pub struct Response {
    /// URL final, depois dos redirecionamentos.
    pub url: Url,
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    /// Cabeçalhos da resposta final (nomes em minúsculas).
    pub headers: Vec<(String, String)>,
}

/// Uma requisição completa (o `fetch` e o XMLHttpRequest das páginas).
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// Envia e guarda cookies.
    pub cookies: bool,
}

impl Request {
    pub fn get(url: &Url) -> Request {
        Request {
            method: "GET".into(),
            url: url.clone(),
            headers: vec![
                ("Accept".into(), "text/html,application/xhtml+xml,text/css,*/*;q=0.8".into()),
                ("Accept-Language".into(), "pt-BR,pt;q=0.9,en;q=0.8".into()),
            ],
            body: None,
            cookies: true,
        }
    }
}

#[derive(Clone)]
pub struct Net {
    agent: ureq::Agent,
    /// Esquema do documento que faz os pedidos (None = o próprio navegador,
    /// que pode tudo). Ver [`Net::for_document`].
    page_scheme: Option<String>,
}

/// Uma página em `from_scheme` pode abrir ou buscar `to`? Página da web não lê
/// nem abre arquivos locais (`file:`) nem as páginas internas (`ferro:`, com o
/// histórico e os favoritos); só uma página do mesmo esquema pode.
pub fn allowed(from_scheme: &str, to: &Url) -> bool {
    match to.scheme() {
        "http" | "https" | "data" => true,
        s @ ("file" | "ferro") => s == from_scheme,
        _ => false,
    }
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

impl Net {
    pub fn new() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .unversioned_rustls_crypto_provider(Arc::new(rustls_rustcrypto::provider()))
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            // redirecionamentos feitos aqui, para tratar os cookies de cada salto
            .max_redirects(0)
            .allow_non_standard_methods(true)
            .user_agent(USER_AGENT)
            .build();
        Net { agent: config.into(), page_scheme: None }
    }

    pub fn fetch(&self, url: &Url) -> Result<Response, String> {
        self.request(&Request::get(url))
    }

    /// Cópia para os pedidos de um documento (scripts, folhas, imagens,
    /// iframes): segue a regra de [`allowed`].
    pub fn for_document(&self, page: &Url) -> Net {
        Net {
            agent: self.agent.clone(),
            page_scheme: Some(page.scheme().to_string()),
        }
    }

    /// Uma requisição http(s), seguindo até 20 redirecionamentos.
    fn http(&self, req: &Request) -> Result<Response, String> {
        use ureq::ResponseExt;
        let mut method = req.method.to_ascii_uppercase();
        let mut url = req.url.clone();
        let mut body = req.body.clone();
        for _ in 0..20 {
            let mut b = ureq::http::Request::builder().method(method.as_str()).uri(url.as_str());
            for (k, v) in &req.headers {
                b = b.header(k.as_str(), v.as_str());
            }
            if req.cookies {
                let c = crate::cookies::header_for(&url);
                if !c.is_empty() {
                    b = b.header("Cookie", c);
                }
            }
            let r = b.body(body.clone().unwrap_or_default()).map_err(|e| e.to_string())?;
            let mut res = self.agent.run(r).map_err(|e| e.to_string())?;
            let final_url = Url::parse(&res.get_uri().to_string()).unwrap_or_else(|_| url.clone());
            let status = res.status().as_u16();
            let headers: Vec<(String, String)> = res
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_ascii_lowercase(), v.to_str().ok()?.to_string())))
                .collect();
            if req.cookies {
                for (_, v) in headers.iter().filter(|(k, _)| k == "set-cookie") {
                    crate::cookies::store_from_header(&final_url, v);
                }
            }
            let location = headers.iter().find(|(k, _)| k == "location").map(|(_, v)| v.clone());
            if matches!(status, 301 | 302 | 303 | 307 | 308)
                && let Some(next) = location.and_then(|l| final_url.join(l.trim()).ok())
            {
                if status == 303 || (matches!(status, 301 | 302) && method == "POST") {
                    method = "GET".into();
                    body = None;
                }
                url = next;
                continue;
            }
            let content_type = headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.clone());
            let body = res
                .body_mut()
                .with_config()
                .limit(50 * 1024 * 1024)
                .read_to_vec()
                .map_err(|e| e.to_string())?;
            return Ok(Response {
                url: final_url,
                status,
                content_type,
                body,
                headers,
            });
        }
        Err("redirecionamentos demais".into())
    }

    pub fn request(&self, req: &Request) -> Result<Response, String> {
        let url = &req.url;
        if let Some(from) = &self.page_scheme
            && matches!(url.scheme(), "file" | "ferro")
            && !allowed(from, url)
        {
            return Err(format!("bloqueado: uma página {from}: não pode abrir {}:", url.scheme()));
        }
        match url.scheme() {
            "http" | "https" if Self::cacheable(req) => self.cached(req),
            "http" | "https" => self.http(req),
            "file" => {
                let path = url.to_file_path().map_err(|_| format!("caminho inválido: {url}"))?;
                let body = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(Response {
                    url: url.clone(),
                    status: 200,
                    content_type: None,
                    body,
                    headers: Vec::new(),
                })
            }
            "data" => {
                let (content_type, body) = decode_data_url(url.as_str())?;
                Ok(Response {
                    url: url.clone(),
                    status: 200,
                    content_type: Some(content_type.clone()),
                    body,
                    headers: vec![("content-type".into(), content_type)],
                })
            }
            other => Err(format!("esquema não suportado: {other}")),
        }
    }
}

impl Net {
    /// GET simples, sem `Range` nem corpo: pode vir do cache.
    fn cacheable(req: &Request) -> bool {
        req.method.eq_ignore_ascii_case("GET")
            && req.body.is_none()
            && !req.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("range") || k.eq_ignore_ascii_case("cache-control"))
            && std::env::var_os("FERRO_SEM_CACHE").is_none()
    }

    /// Requisição passando pelo cache HTTP.
    fn cached(&self, req: &Request) -> Result<Response, String> {
        use crate::httpcache::{self, Lookup};
        match httpcache::lookup(&req.url) {
            Lookup::Fresh(r) => Ok(r),
            Lookup::Stale(cond) => {
                let mut r2 = req.clone();
                r2.headers.extend(cond);
                let res = self.http(&r2)?;
                if res.status == 304
                    && let Some(r) = httpcache::revalidated(&req.url, &res.headers)
                {
                    return Ok(r);
                }
                httpcache::store(&req.url, &res);
                Ok(res)
            }
            Lookup::Miss => {
                let res = self.http(req)?;
                httpcache::store(&req.url, &res);
                Ok(res)
            }
        }
    }
}

/// data:[tipo][;base64],dados
fn decode_data_url(s: &str) -> Result<(String, Vec<u8>), String> {
    let rest = s.strip_prefix("data:").ok_or("não é data: URL")?;
    let (meta, data) = rest.split_once(',').ok_or("data: URL sem vírgula")?;
    let is_b64 = meta.ends_with(";base64");
    let mime = meta.trim_end_matches(";base64");
    let mime = if mime.is_empty() { "text/plain;charset=US-ASCII" } else { mime };
    let raw = percent_decode(data);
    let body = if is_b64 { base64_decode(&raw)? } else { raw };
    Ok((mime.to_string(), body))
}

pub fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn base64_decode(input: &[u8]) -> Result<Vec<u8>, String> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in input {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c).ok_or("base64 inválido")?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Bytes 0x80–0x9F do Windows-1252 (o resto coincide com Latin-1).
const CP1252: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}',
    '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

fn charset_of(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let pos = lower.find("charset=")?;
    let cs: String = lower[pos + 8..]
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if cs.is_empty() { None } else { Some(cs) }
}

/// Decodifica o corpo em texto: BOM → cabeçalho HTTP → <meta charset> → UTF-8.
pub fn decode_text(body: &[u8], content_type: Option<&str>, sniff_meta: bool) -> String {
    if let Some(rest) = body.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let mut charset = content_type.and_then(charset_of);
    if charset.is_none() && sniff_meta {
        let head = String::from_utf8_lossy(&body[..body.len().min(4096)]);
        charset = charset_of(&head);
    }
    match charset.as_deref() {
        Some("iso-8859-1" | "latin1" | "windows-1252" | "cp1252" | "us-ascii" | "iso-8859-15" | "ascii") => body
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => CP1252[(b - 0x80) as usize],
                _ => b as char,
            })
            .collect(),
        _ => String::from_utf8_lossy(body).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_nao_le_arquivos_nem_paginas_internas() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(allowed("https", &u("https://example.com/")));
        assert!(allowed("https", &u("data:text/plain,oi")));
        assert!(!allowed("https", &u("file:///C:/Windows/win.ini")));
        assert!(!allowed("https", &u("ferro://historico")));
        assert!(!allowed("file", &u("ferro://historico/limpar")));
        assert!(allowed("file", &u("file:///C:/x.txt")));
        assert!(allowed("ferro", &u("ferro://historico/limpar")));
        assert!(!allowed("https", &u("javascript:alert(1)")));
        // o mesmo vale para os pedidos feitos com a cópia da página
        let net = Net::new().for_document(&u("https://example.com/"));
        assert!(net.fetch(&u("ferro://historico")).err().is_some_and(|e| e.contains("bloqueado")));
        assert!(net.fetch(&u("file:///C:/Windows/win.ini")).err().is_some_and(|e| e.contains("bloqueado")));
    }

    #[test]
    fn data_url() {
        let (t, b) = decode_data_url("data:text/html;base64,PGgxPk9pPC9oMT4=").unwrap();
        assert_eq!(t, "text/html");
        assert_eq!(b, b"<h1>Oi</h1>");
        let (_, b) = decode_data_url("data:,Ol%C3%A1").unwrap();
        assert_eq!(String::from_utf8(b).unwrap(), "Olá");
    }

    #[test]
    fn latin1() {
        let s = decode_text(b"S\xe3o Paulo \x93ok\x94", Some("text/html; charset=ISO-8859-1"), false);
        assert_eq!(s, "São Paulo “ok”");
        let s = decode_text(b"<meta charset=\"windows-1252\">a\xe7\xe3o", None, true);
        assert!(s.ends_with("ação"));
    }
}
