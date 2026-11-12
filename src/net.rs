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

