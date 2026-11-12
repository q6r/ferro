//! Páginas internas feitas em HTML e desenhadas pelo próprio motor:
//! `ferro://historico` e `ferro://favoritos` (dados do perfil, `src/dados.rs`).

use std::sync::atomic::{AtomicBool, Ordering};
use url::Url;

static DARK: AtomicBool = AtomicBool::new(false);

/// A interface avisa se o tema atual é escuro (as páginas seguem).
pub fn set_dark(dark: bool) {
    DARK.store(dark, Ordering::Relaxed);
}

/// Cores da página: claras ou escuras, conforme o tema da interface.
fn palette() -> &'static str {
    if DARK.load(Ordering::Relaxed) {
        ":root { --fundo: #1b1a19; --texto: #ece8e3; --apagado: #9a918a; --lista: #252321; --linha: #302d2a; \
         --borda: #3a3633; --sobre: #2f2c29; --link: #e8875f; --campo: #252321; }"
    } else {
        ":root { --fundo: #f6f5f3; --texto: #1f1f1f; --apagado: #8a8178; --lista: #ffffff; --linha: #f0ece6; \
         --borda: #e6e2dc; --sobre: #f3efe9; --link: #b44b2a; --campo: #ffffff; }"
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const STYLE: &str = r#"
<style>
  body { font-family: system-ui, sans-serif; margin: 0; background: var(--fundo); color: var(--texto); }
  main { max-width: 860px; margin: 0 auto; padding: 32px 28px 60px; }
  h1 { font-size: 28px; margin: 0 0 6px; }
  .sub { color: var(--apagado); margin: 0 0 22px; font-size: 14px; }
  .busca { width: 100%; box-sizing: border-box; padding: 11px 16px; font-size: 15px; border: 1px solid var(--borda);
           border-radius: 12px; background: var(--campo); color: var(--texto); font-family: inherit; }
  .caixa { margin-bottom: 18px; }
  h2 { font-size: 13px; text-transform: uppercase; letter-spacing: .06em; color: var(--apagado); margin: 26px 0 8px; }
  .lista { background: var(--lista); border: 1px solid var(--borda); border-radius: 14px; overflow: hidden; }
  .item { display: flex; align-items: center; gap: 14px; padding: 10px 16px; border-top: 1px solid var(--linha);
          text-decoration: none; color: inherit; }
  .item:first-child { border-top: 0; }
  .item:hover { background: var(--sobre); }
  .hora { color: var(--apagado); font-size: 12px; width: 44px; flex: none; }
  .titulo { flex: 1; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; font-size: 14px; }
  .dominio { color: var(--apagado); font-size: 12px; max-width: 220px; overflow: hidden; white-space: nowrap; }
  .vazio { color: var(--apagado); padding: 40px 0; text-align: center; }
  .acoes { margin-top: 22px; font-size: 13px; }
  .acoes a { color: var(--link); }
</style>
<script>
  function filtrar(q) {
    q = q.toLowerCase();
    for (const a of document.querySelectorAll('.item')) {
      a.style.display = a.textContent.toLowerCase().includes(q) ? '' : 'none';
    }
  }
</script>
"#;

/// HTML de uma página interna (None se não existe).
pub fn page(url: &Url) -> Option<String> {
    match url.host_str()? {
        "historico" => {
            if url.path() == "/limpar" {
                crate::dados::clear_history();
            }
            Some(history())
        }
        "favoritos" => Some(bookmarks()),
        _ => None,
    }
}

fn domain(u: &str) -> String {
    Url::parse(u).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string())).unwrap_or_default()
}

fn history() -> String {
    let list = crate::dados::read_history();
    let mut body = String::new();
    let mut last_day = String::new();
    let mut seen = std::collections::HashSet::new();
    let tz = jiff::tz::TimeZone::system();
    let today = jiff::Zoned::now().date();
    for (when, url, title) in list.iter().take(1500) {
        // a mesma página no mesmo dia aparece uma vez só
        let Ok(ts) = jiff::Timestamp::from_second(*when as i64) else { continue };
        let z = ts.to_zoned(tz.clone());
        let day = z.date();
        if !seen.insert((day, url.clone())) {
            continue;
        }
        let label = if day == today {
            "Hoje".to_string()
        } else if Some(day) == today.yesterday().ok() {
            "Ontem".to_string()
        } else {
            format!("{:02}/{:02}/{}", day.day(), day.month(), day.year())
        };
        if label != last_day {
            if !last_day.is_empty() {
                body.push_str("</div>");
            }
            body.push_str(&format!("<h2>{}</h2><div class=lista>", esc(&label)));
            last_day = label;
        }
        let title = if title.trim().is_empty() { url.as_str() } else { title.as_str() };
        body.push_str(&format!(
            "<a class=item href=\"{}\"><span class=hora>{:02}:{:02}</span><span class=titulo>{}</span><span class=dominio>{}</span></a>",
            esc(url),
            z.hour(),
            z.minute(),
            esc(title),
            esc(&domain(url))
        ));
    }
    let pal = palette();
    if !last_day.is_empty() {
        body.push_str("</div><p class=acoes><a href=\"ferro://historico/limpar\">Apagar o histórico</a></p>");
    } else {
        body.push_str("<p class=vazio>Nenhuma página visitada ainda.</p>");
    }
    format!(
        "<!DOCTYPE html><html lang=pt-BR><head><meta charset=utf-8><title>Histórico</title><style>{pal}</style>{STYLE}</head><body><main>\
         <h1>Histórico</h1><p class=sub>As páginas que você abriu no Ferro (Ctrl+H).</p>\
         <div class=caixa><input class=busca placeholder=\"Buscar no histórico\" oninput=\"filtrar(this.value)\"></div>{body}</main></body></html>"
    )
}

fn bookmarks() -> String {
    let list = crate::dados::read_bookmarks();
    let pal = palette();
    let mut body = String::new();
    if list.is_empty() {
        body.push_str("<p class=vazio>Nenhum favorito ainda. Use Ctrl+D numa página para salvar.</p>");
    } else {
        body.push_str("<div class=lista>");
        for (url, title) in list.iter().rev() {
            let title = if title.trim().is_empty() { url.as_str() } else { title.as_str() };
            body.push_str(&format!(
                "<a class=item href=\"{}\"><span class=titulo>{}</span><span class=dominio>{}</span></a>",
                esc(url),
                esc(title),
                esc(&domain(url))
            ));
        }
        body.push_str("</div>");
    }
    format!(
        "<!DOCTYPE html><html lang=pt-BR><head><meta charset=utf-8><title>Favoritos</title><style>{pal}</style>{STYLE}</head><body><main>\
         <h1>Favoritos</h1><p class=sub>Páginas salvas com Ctrl+D (ou pelo menu do botão direito).</p>\
         <div class=caixa><input class=busca placeholder=\"Buscar nos favoritos\" oninput=\"filtrar(this.value)\"></div>{body}</main></body></html>"
    )
}
