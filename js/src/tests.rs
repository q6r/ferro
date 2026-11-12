//! Testes de ponta a ponta: scripts JavaScript e a saída do console.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{Engine, Value};

/// Roda o script e devolve tudo o que foi impresso (uma linha por console.log).
/// Usa uma thread com pilha grande, como o navegador faz.
fn run(src: &str) -> String {
    let src = src.to_string();
    std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(move || run_here(&src))
        .unwrap()
        .join()
        .unwrap()
}

fn run_here(src: &str) -> String {
    let out = Rc::new(RefCell::new(Vec::<String>::new()));
    let mut e = Engine::new();
    let sink = out.clone();
    e.set_output(move |_, s| sink.borrow_mut().push(s.to_string()));
    if let Err(err) = e.eval(src, "teste.js") {
        let t = e.error_text(&err);
        out.borrow_mut().push(t);
    }
    e.run_event_loop();
    let lines = out.borrow().join("\n");
    lines
}

fn check(src: &str, expected: &str) {
    let got = run(src);
    assert_eq!(got.trim(), expected.trim(), "\n--- script:\n{src}\n");
}

#[test]
fn aritmetica_e_strings() {
    check(
        r#"
        console.log(1 + 2 * 3, 7 / 2, 7 % 3, -7 % 3, 2 ** 10, 0.1 + 0.2);
        console.log("a" + 1 + 2, 1 + 2 + "a", "5" * "2", "5" - 2, +"", +" 12 ");
        console.log(typeof 1, typeof "x", typeof null, typeof undefined, typeof {}, typeof function(){}, typeof Symbol());
        console.log(1 / 0, -1 / 0, 0 / 0, 5 | 3, 5 & 3, 5 ^ 3, ~5, 1 << 31, -1 >>> 0, -16 >> 2);
        console.log(null == undefined, null === undefined, "1" == 1, NaN == NaN, [1] == 1, "b" > "a");
        "#,
        "7 3.5 1 -1 1024 0.30000000000000004\na12 3a 10 3 0 12\nnumber string object undefined object function symbol\nInfinity -Infinity NaN 7 1 6 -6 -2147483648 4294967295 -4\ntrue false true false true true",
    );
}

#[test]
fn variaveis_escopos_closures() {
    check(
        r#"
        let x = 1;
        { let x = 2; console.log(x); }
        console.log(x);
        function contador() { let n = 0; return () => ++n; }
        const c = contador(); c(); c();
        console.log(c());
        var fs = [];
        for (let i = 0; i < 3; i++) fs.push(() => i);
        console.log(fs.map(f => f()).join(","));
        var gs = [];
        for (var j = 0; j < 3; j++) gs.push(() => j);
        console.log(gs.map(f => f()).join(","));
        console.log(typeof naoExiste, (function() { return typeof arguments; })());
        try { y; let y = 1; } catch (e) { console.log(e.name, e.message); }
        const k = 5;
        try { k = 6; } catch (e) { console.log(e.message); }
        console.log(hoisted(), typeof depois);
        function hoisted() { return "ok"; }
        var depois = 1;
        "#,
        "2\n1\n3\n0,1,2\n3,3,3\nundefined object\nReferenceError Cannot access 'y' before initialization\nAssignment to constant variable.\nok undefined",
    );
}

