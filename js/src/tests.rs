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

#[test]
fn controle_de_fluxo() {
    check(
        r#"
        let s = "";
        outer: for (let i = 0; i < 4; i++) {
            for (let j = 0; j < 4; j++) {
                if (j === 2) continue outer;
                if (i === 3) break outer;
                s += i + "" + j + " ";
            }
        }
        console.log(s.trim());
        let n = 0; while (true) { if (++n > 5) break; } console.log(n);
        let d = 0; do { d++; } while (d < 3); console.log(d);
        function f(x) {
            switch (x) { case 1: return "um"; case 2: case 3: return "dois-tres"; default: return "outro"; }
        }
        console.log(f(1), f(3), f(9));
        function g() { try { return "try"; } finally { console.log("finally"); } }
        console.log(g());
        function h() { for (const x of [1, 2, 3]) { try { if (x == 2) return x; } finally { console.log("f" + x); } } }
        console.log(h());
        try { throw new TypeError("ruim"); } catch ({ name, message }) { console.log(name + ": " + message); }
        const o = { a: 1, b: 2 }; const ks = []; for (const k in o) ks.push(k); console.log(ks.join());
        console.log(true ? "sim" : "não", null ?? "padrão", 0 || "falso", 1 && "verdadeiro");
        "#,
        "00 01 10 11 20 21\n6\n3\num dois-tres outro\nfinally\ntry\nf1\nf2\n2\nTypeError: ruim\na,b\nsim padrão falso verdadeiro",
    );
}

#[test]
fn objetos_arrays_desestruturacao() {
    check(
        r#"
        const { a, b: { c = 5 } = {}, ...resto } = { a: 1, d: 4, e: 5 };
        console.log(a, c, resto);
        const [p, , q = 10, ...r] = [1, 2, undefined, 4, 5];
        console.log(p, q, r);
        let m = 1, n = 2; [m, n] = [n, m]; console.log(m, n);
        const obj = { x: 1, get dobro() { return this.x * 2; }, ["k" + 1]: "computada", metodo() { return "m"; } };
        console.log(obj.dobro, obj.k1, obj.metodo(), Object.keys(obj));
        const arr = [5, 1, 4];
        console.log([...arr, ...[9]], Math.max(...arr), arr.sort(), arr.length);
        console.log({ ...obj, x: 9 }.x, Object.entries({ u: 1 }));
        console.log([1, [2, [3, [4]]]].flat(Infinity), Array.from({ length: 3 }, (_, i) => i * i));
        console.log([3, 1, 2].map(x => x * 2).filter(x => x > 2).reduce((s, x) => s + x, 0));
        console.log(arr.includes(4), arr.indexOf(5), [1, 2, 3].at(-1), "abc".at(-1));
        const nested = { a: [1, { b: 2 }], s: "texto" };
        console.log(nested);
        console.log(JSON.stringify(nested), JSON.parse('{"x":[1,2,{"y":null}]}').x[2].y);
        console.log(JSON.stringify({ a: 1, b: [1, 2] }, null, 2));
        "#,
        "1 5 { d: 4, e: 5 }\n1 10 [ 4, 5 ]\n2 1\n2 computada m [ 'x', 'dobro', 'k1', 'metodo' ]\n[ 5, 1, 4, 9 ] 5 [ 1, 4, 5 ] 3\n9 [ [ 'u', 1 ] ]\n[ 1, 2, 3, 4 ] [ 0, 1, 4 ]\n10\ntrue 2 3 c\n{ a: [ 1, { b: 2 } ], s: 'texto' }\n{\"a\":[1,{\"b\":2}],\"s\":\"texto\"} null\n{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2\n  ]\n}",
    );
}

#[test]
fn classes() {
    check(
        r#"
        class Animal {
            #nome;
            static total = 0;
            constructor(nome) { this.#nome = nome; Animal.total++; }
            get nome() { return this.#nome; }
            fala() { return `${this.#nome} faz barulho`; }
            static criar(n) { return new this(n); }
            #segredo() { return "s"; }
            revela() { return this.#segredo(); }
            static #privado = 42;
            static lerPrivado() { return Animal.#privado; }
        }
        class Cao extends Animal {
            raca = "vira-lata";
            constructor(nome) { super(nome); }
            fala() { return super.fala() + " (au)"; }
        }
        const d = new Cao("Rex");
        console.log(d.fala(), d.nome, d.raca, Animal.total, d instanceof Animal, d.revela());
        console.log(Cao.criar("Bob").nome, Animal.total, Animal.lerPrivado());
        class Ponto { constructor(x, y) { this.x = x; this.y = y; } toString() { return `(${this.x}, ${this.y})`; } }
        console.log(String(new Ponto(1, 2)), `${new Ponto(3, 4)}`, new Ponto(5, 6));
        try { Animal(); } catch (e) { console.log(e.message); }
        class Erro extends Error { constructor(m) { super(m); this.name = "Erro"; } }
        const e = new Erro("falhou");
        console.log(e instanceof Error, e.name, e.message, String(e));
        class A { static { A.iniciado = true; } }
        console.log(A.iniciado, typeof A, Object.getPrototypeOf(Cao) === Animal);
        "#,
        "Rex faz barulho (au) Rex vira-lata 1 true s\nBob 2 42\n(1, 2) (3, 4) Ponto { x: 5, y: 6 }\nClass constructor Animal cannot be invoked without 'new'\ntrue Erro falhou Erro: falhou\ntrue function true",
    );
}

#[test]
fn funcoes_this_bind() {
    check(
        r#"
        function soma(...xs) { return xs.reduce((a, b) => a + b, 0); }
        console.log(soma(1, 2, 3), soma.apply(null, [4, 5]), soma.call(null, 6), soma.length, soma.name);
        const o = { v: 10, f() { return this.v; }, g: () => typeof this };
        const solto = o.f;
        console.log(o.f(), solto.call({ v: 20 }), o.f.bind({ v: 30 })(), o.g());
        function padrao(a, b = a * 2, { c } = { c: 3 }) { return [a, b, c]; }
        console.log(padrao(1), padrao(1, 5, { c: 7 }));
        const fat = function f(n) { return n <= 1 ? 1 : n * f(n - 1); };
        console.log(fat(10), (x => y => x + y)(1)(2));
        function Pessoa(nome) { this.nome = nome; }
        Pessoa.prototype.oi = function () { return "oi " + this.nome; };
        console.log(new Pessoa("Ana").oi(), new Pessoa("Bia") instanceof Pessoa);
        console.log(new Function("a", "b", "return a * b")(6, 7));
        const tag = (s, ...v) => s.raw.join("|") + v.join(",");
        console.log(tag`a${1}b${2}c`);
        console.log([1, 2, 3].map(String), ["1", "2", "3"].map(Number), ["a", "b"].map((x, i) => x + i));
        "#,
        "6 9 6 0 soma\n10 20 30 object\n[ 1, 2, 3 ] [ 1, 5, 7 ]\n3628800 3\noi Ana true\n42\na|b|c1,2\n[ '1', '2', '3' ] [ 1, 2, 3 ] [ 'a0', 'b1' ]",
    );
}

