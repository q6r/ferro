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

#[test]
fn clique_link_e_eventos() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0">
            <a id=l href="https://exemplo.com/x" style="display:block;height:50px">ir</a>
            <div id=d style="height:50px" onclick="this.textContent='clicado'">d</div>
            <a id=p href="/y" style="display:block;height:50px"><span>não</span></a>
            <script>
              window.log = [];
              document.getElementById('p').addEventListener('click', e => { log.push('p'); e.preventDefault(); });
              document.addEventListener('mousedown', e => log.push('down:' + e.target.id));
            </script></body>"#,
        );
        // link comum: o clique vira navegação
        let nav = s.click(10.0, 20.0);
        assert_eq!(nav.map(|u| u.url.to_string()), Some("https://exemplo.com/x".into()));
        // div com onclick inline: muda o DOM, sem navegar
        assert!(s.click(10.0, 70.0).is_none());
        assert_eq!(eval(&mut s, "document.getElementById('d').textContent"), "clicado");
        // link com preventDefault: não navega (o alvo é o <span> dentro dele)
        assert!(s.click(5.0, 106.0).is_none());
        assert_eq!(eval(&mut s, "log.join()"), "down:l,down:d,down:,p");
        // o alvo do último clique foi o <span> (sem id)
        assert_eq!(eval(&mut s, "document.elementFromPoint(5, 106).localName"), "span");
    });
}

#[test]
fn location_e_formulario() {
    with_stack(|| {
        let mut s = session(
            r#"<form id=f action="/busca"><input name=q value="a b"><button id=b>ir</button></form>
            <script>location.hash = 'sec';</script>"#,
        );
        // só o fragmento: não recarrega
        assert!(s.take_navigation().is_none());
        assert_eq!(eval(&mut s, "location.href"), "https://teste.ferro/pagina#sec");
        eval(&mut s, "document.getElementById('b').click()");
        assert_eq!(eval(&mut s, "1"), "1");
        let nav = s.take_navigation().map(|u| u.url.to_string());
        assert_eq!(nav, Some("https://teste.ferro/busca?q=a+b".into()));
        eval(&mut s, "location.href = '/outra'");
        assert_eq!(s.take_navigation().map(|u| u.url.to_string()), Some("https://teste.ferro/outra".into()));
        eval(&mut s, "history.pushState({}, '', '/spa/1')");
        assert_eq!(eval(&mut s, "location.pathname"), "/spa/1");
        assert!(s.take_navigation().is_none());
    });
}

