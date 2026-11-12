//! Testes da ligação JS ↔ DOM (sessões montadas a partir de HTML em memória).

use std::rc::Rc;

use url::Url;

use super::{STACK_SIZE, Session};
use crate::engine::Loaded;
use crate::fonts::Fonts;
use crate::net::Net;

/// Roda `f` numa thread com pilha grande (o interpretador é recursivo).
fn with_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(STACK_SIZE).spawn(f).unwrap().join().unwrap();
}

fn session(html: &str) -> Session {
    let url = Url::parse("https://teste.ferro/pagina").unwrap();
    let doc = crate::html::parse(html);
    let loaded = Loaded {
        url: url.clone(),
        base: url,
        status: 200,
        doc,
        css: Vec::new(),
        title: String::new(),
        sheet_texts: Default::default(),
    };
    let fonts = Rc::new(Fonts::system().expect("fontes do sistema"));
    let mut s = Session::new(loaded, Net::new(), fonts, 800.0, 600.0);
    s.disable_resources();
    // as <style> do HTML (o carregamento normal já as junta)
    s.host.borrow_mut().styles_dirty = true;
    let _ = s.render();
    s.run_scripts(&|| false);
    s
}

fn eval(s: &mut Session, code: &str) -> String {
    match s.js.eval(code, "teste") {
        Ok(v) => match v {
            ferro_js::Value::String(t) => t.to_rust(),
            v => s.js.inspect(&v),
        },
        Err(e) => s.js.error_text(&e),
    }
}

