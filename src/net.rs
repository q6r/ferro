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
        Net { agent: config.into() }
    }

    pub fn fetch(&self, url: &Url) -> Result<Response, String> {
        self.request(&Request::get(url))
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

