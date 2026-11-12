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

#[test]
fn colecoes_iteradores_simbolos() {
    check(
        r#"
        const m = new Map([["a", 1], ["b", 2]]);
        m.set("c", 3).delete("a");
        console.log(m.size, m.get("b"), [...m.keys()], m);
        const s = new Set([1, 2, 2, 3]);
        console.log(s.size, s.has(2), [...s].join("-"), s);
        const it = { from: 1, to: 3, [Symbol.iterator]() { let c = this.from, t = this.to; return { next: () => c <= t ? { value: c++, done: false } : { value: undefined, done: true } }; } };
        console.log([...it], Array.from(it).length);
        for (const [k, v] of m) console.log(k, v);
        for (const ch of "aé😀") console.log(ch, ch.length);
        const sym = Symbol("meu");
        const o = { [sym]: 1, normal: 2 };
        console.log(sym.toString(), sym.description, Object.keys(o), o[sym], Symbol.for("x") === Symbol.for("x"));
        const wm = new WeakMap(); const k = {}; wm.set(k, "v"); console.log(wm.get(k), wm.has({}));
        "#,
        "2 2 [ 'b', 'c' ] Map(2) { 'b' => 2, 'c' => 3 }\n3 true 1-2-3 Set(3) { 1, 2, 3 }\n[ 1, 2, 3 ] 3\nb 2\nc 3\na 1\né 1\n😀 2\nSymbol(meu) meu [ 'normal' ] 1 true\nv false",
    );
}

#[test]
fn metodos_de_texto_e_numero() {
    check(
        r#"
        console.log("Olá Mundo".toUpperCase(), "ABC".toLowerCase(), "  x ".trim() + "|", "a-b-c".split("-"), "abc".split(""));
        console.log("abcdef".slice(1, -1), "abcdef".substring(4, 1), "abc".padStart(5, "*"), "ab".repeat(3), "a,b".replace(",", ";"));
        console.log("x".charCodeAt(0), String.fromCharCode(72, 105), "abc".indexOf("c"), "aaa".lastIndexOf("a"), "abc".includes("bc"));
        console.log((255).toString(16), (3.14159).toFixed(2), (1234.5678).toPrecision(6), (0.000001234).toExponential(2), (1234567.891).toLocaleString());
        console.log(parseInt("42px"), parseFloat("3.5e2x"), Number("0b101"), Number.isInteger(5.0), isNaN("abc"), Number.MAX_SAFE_INTEGER);
        console.log(Math.round(2.5), Math.round(-2.5), Math.floor(-1.5), Math.trunc(-1.5), Math.sign(-3), Math.abs(-7), Math.hypot(3, 4));
        console.log("a-b-c".replaceAll("-", "+"), "Hello".startsWith("He"), "Hello".endsWith("lo"), `linha1
linha2`.split("\n").length);
        console.log(encodeURIComponent("a b&c/é"), decodeURIComponent("a%20b%C3%A9"), escape("a b"));
        "#,
        "OLÁ MUNDO abc x| [ 'a', 'b', 'c' ] [ 'a', 'b', 'c' ]\nbcde bcd **abc ababab a;b\n120 Hi 2 2 true\nff 3.14 1234.57 1.23e-6 1,234,567.891\n42 350 5 true true 9007199254740991\n3 -2 -2 -1 -1 7 5\na+b+c true true 2\na%20b%26c%2F%C3%A9 a bé a%20b",
    );
}

#[test]
fn erros_e_pilha() {
    let out = run(
        r#"
        function a() { b(); }
        function b() { null.x; }
        try { a(); } catch (e) { console.log(e.constructor.name); console.log(e.message); console.log(e.stack.split("\n").length > 2); }
        try { undefinedFn(); } catch (e) { console.log(e.message); }
        try { const o = {}; o.metodo(); } catch (e) { console.log(e.message); }
        try { JSON.parse("{x"); } catch (e) { console.log(e.name); }
        try { new Array(-1); } catch (e) { console.log(e.name, e.message); }
        function rec() { return rec(); }
        try { rec(); } catch (e) { console.log(e.name, e.message); }
        throw new Error("fim");
        "#,
    );
    assert_eq!(
        out.lines().take(8).collect::<Vec<_>>().join("\n"),
        "TypeError\nCannot read properties of null (reading 'x')\ntrue\nundefinedFn is not defined\no.metodo is not a function\nSyntaxError\nRangeError Invalid array length\nRangeError Maximum call stack size exceeded"
    );
    assert!(out.contains("Uncaught Error: fim"), "{out}");
    assert!(out.contains("at teste.js:"), "{out}");
}

#[test]
fn getters_setters_defineproperty() {
    check(
        r#"
        const o = {};
        Object.defineProperty(o, "fixo", { value: 1, writable: false, enumerable: false });
        o.fixo = 2;
        console.log(o.fixo, Object.keys(o), JSON.stringify(Object.getOwnPropertyDescriptor(o, "fixo")));
        "use strict";
        const p = { _v: 0, get v() { return this._v; }, set v(x) { this._v = x * 2; } };
        p.v = 5; console.log(p.v);
        const f = Object.freeze({ a: 1 }); f.a = 9; console.log(f.a, Object.isFrozen(f));
        const herda = Object.create({ base: "b" }, { proprio: { value: "p", enumerable: true } });
        console.log(herda.base, herda.proprio, "base" in herda, herda.hasOwnProperty("base"));
        console.log(Object.getOwnPropertyNames([1, 2]), Reflect.ownKeys({ a: 1, [Symbol.iterator]: 2 }).length);
        console.log(Object.prototype.toString.call([]), Object.prototype.toString.call(null), String({}));
        "#,
        "1 [] {\"value\":1,\"writable\":false,\"enumerable\":false,\"configurable\":false}\n10\n1 true\nb p true false\n[ '0', '1', 'length' ] 2\n[object Array] [object Null] [object Object]",
    );
}

#[test]
fn datas() {
    check(
        r#"
        const d = new Date(Date.UTC(2024, 0, 2, 10, 30));
        console.log(d.toISOString(), d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDay(), d.getTime());
        console.log(new Date("2024-03-05T12:00:00Z").getUTCDate(), JSON.stringify({ d }), isNaN(new Date("lixo")));
        "#,
        "2024-01-02T10:30:00.000Z 2024 0 2 1704191400000\n5 {\"d\":\"2024-01-02T10:30:00.000Z\"} true",
    );
}

#[test]
fn inspect_estilo_node() {
    check(
        r#"
        console.log([1, "a", null, undefined, true], { "chave-estranha": 1, ok: [] }, [ , 1]);
        console.log(function nomeada() {}, class X {}, () => {}, Symbol("s"), -0, [new Error("e").message]);
        console.log({ a: { b: { c: { d: 1 } } } }, new Map(), new Set(), Object.create(null));
        console.log(Array.from({ length: 30 }, (_, i) => i));
        const ciclo = { nome: "c" }; ciclo.eu = ciclo; console.log(ciclo);
        console.log("%s tem %d anos e %o", "Ana", 30.7, { x: 1 }, "extra");
        "#,
        "[ 1, 'a', null, undefined, true ] { 'chave-estranha': 1, ok: [] } [ <1 empty item>, 1 ]\n[Function: nomeada] [class X] [Function (anonymous)] Symbol(s) -0 [ 'e' ]\n{ a: { b: { c: [Object] } } } Map(0) {} Set(0) {} [Object: null prototype] {}\n[\n   0,  1,  2,  3,  4,  5,  6,  7,  8,\n   9, 10, 11, 12, 13, 14, 15, 16, 17,\n  18, 19, 20, 21, 22, 23, 24, 25, 26,\n  27, 28, 29\n]\n<ref *1> { nome: 'c', eu: [Circular *1] }\nAna tem 30.7 anos e { x: 1 } extra",
    );
}

#[test]
fn programa_maior() {
    // um "mini app": fila de tarefas, ordenação e agregação
    check(
        r#"
        const tarefas = [
            { id: 1, titulo: "Comprar pão", prioridade: 2, feita: false, tags: ["casa"] },
            { id: 2, titulo: "Estudar Rust", prioridade: 1, feita: true, tags: ["estudo", "rust"] },
            { id: 3, titulo: "Correr", prioridade: 3, feita: false, tags: ["saude"] },
            { id: 4, titulo: "Ler", prioridade: 1, feita: false, tags: ["estudo"] },
        ];
        const porTag = tarefas.reduce((acc, t) => {
            for (const tag of t.tags) (acc[tag] ??= []).push(t.id);
            return acc;
        }, {});
        console.log(porTag);
        const pendentes = tarefas.filter(t => !t.feita).sort((a, b) => a.prioridade - b.prioridade || a.titulo.localeCompare(b.titulo));
        console.log(pendentes.map(({ titulo, prioridade }) => `${prioridade}:${titulo}`).join(" | "));
        class Fila {
            #itens = [];
            entra(x) { this.#itens.push(x); return this; }
            sai() { return this.#itens.shift(); }
            get tamanho() { return this.#itens.length; }
        }
        const f = new Fila().entra("a").entra("b");
        console.log(f.sai(), f.tamanho);
        "#,
        "{ casa: [ 1 ], estudo: [ 2, 4 ], rust: [ 2 ], saude: [ 3 ] }
1:Ler | 2:Comprar pão | 3:Correr
a 1",
    );
}

#[test]
fn geradores_promessas_async() {
    check(
        r#"
        function* g() { const x = yield 1; yield x * 2; }
        const it = g(); console.log(it.next().value, it.next(21).value, it.next().done);
        async function f() { await null; return "async"; }
        f().then(v => console.log(v));
        Promise.all([1, Promise.resolve(2)]).then(v => console.log(v));
        setTimeout(() => console.log("timer"), 1);
        (async () => { for await (const x of (async function* () { yield "ag1"; yield "ag2"; })()) console.log(x); })();
        console.log("sync");
        "#,
        "1 42 true\nsync\nasync\n[ 1, 2 ]\nag1\nag2\ntimer",
    );
}

/// Módulos: dois módulos ligados à mão (como o navegador faz), com ciclo,
/// ligação viva, default, namespace, reexportação e await no topo.
#[test]
fn modulos_es() {
    let out = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(|| {
            let lines = Rc::new(RefCell::new(Vec::<String>::new()));
            let mut e = Engine::new();
            let sink = lines.clone();
            e.set_output(move |_, s| sink.borrow_mut().push(s.to_string()));
            e.eval(
                r#"globalThis.__ferroExport = (ns, n, get) => Object.defineProperty(ns, n, { get, enumerable: true });
                   globalThis.__ferroExportAll = (ns, src) => { for (const k of Object.keys(src)) if (k !== 'default' && !(k in ns)) __ferroExport(ns, k, () => src[k]); };"#,
                "prep",
            )
            .ok();
            let (req_a, fa) = crate::module::module_function(
                &mut e.vm,
                r#"import def, { contador, inc as somar } from "./b.js";
                   import * as tudo from "./b.js";
                   export let nome = "a";
                   export function dobro(x) { return x * 2; }
                   export * from "./b.js";
                   console.log("a:", def(), contador, typeof somar);
                   somar(); somar();
                   console.log("viva:", contador, tudo.contador, Object.keys(tudo).join());
                   const v = await Promise.resolve(42);
                   console.log("await:", v, import.meta.url);"#,
                "https://x/a.js",
            )
            .unwrap_or_else(|err| panic!("{}", e.error_text(&err)));
            let (req_b, fb) = crate::module::module_function(
                &mut e.vm,
                r#"import { dobro } from "./a.js";
                   export let contador = 0;
                   export function inc() { contador++; }
                   export default function () { return "padrão:" + dobro(21); }
                   export { inc as mais };"#,
                "https://x/b.js",
            )
            .unwrap_or_else(|err| panic!("{}", e.error_text(&err)));
            assert_eq!(req_a, vec!["./b.js"]);
            assert_eq!(req_b, vec!["./a.js"]);
            let ns_a = e.vm.eval_script("Object.create(null)", "ns").ok().unwrap();
            let ns_b = e.vm.eval_script("Object.create(null)", "ns").ok().unwrap();
            let meta = e.vm.eval_script("({ url: 'https://x/a.js' })", "meta").ok().unwrap();
            // b primeiro (dependência), depois a; o ciclo a↔b funciona porque
            // `dobro` só é usado quando a função default roda
            if let Err(err) = e.vm.call(&fb, Value::Undefined, vec![ns_b.clone(), ns_a.clone(), Value::Undefined]) {
                panic!("{}", e.error_text(&err));
            }
            if let Err(err) = e.vm.call(&fa, Value::Undefined, vec![ns_a.clone(), ns_b.clone(), meta]) {
                panic!("{}", e.error_text(&err));
            }
            e.run_event_loop();
            let l = lines.borrow().join("\n");
            l
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(
        out,
        "a: padrão:42 0 function\nviva: 2 2 contador,inc,default,mais\nawait: 42 https://x/a.js"
    );
}

/// Coletor de ciclos: ciclos sem dono somem; o que está em uso continua inteiro.
#[test]
fn coletor_de_ciclos() {
    let out = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(|| {
            let lines = Rc::new(RefCell::new(Vec::<String>::new()));
            let mut e = Engine::new();
            let sink = lines.clone();
            e.set_output(move |_, s| sink.borrow_mut().push(s.to_string()));
            let _ = crate::gc::collect();
            let base = crate::gc::tracked();
            e.eval(
                r#"
                globalThis.keep = { name: 'vivo' }; keep.self = keep;
                globalThis.cls = (function () { let n = 0; const f = () => ++n; f.again = f; return f; })();
                const m = new Map(); m.set(keep, cls); keep.m = m;
                globalThis.wm = new WeakMap(); globalThis.chave = {}; wm.set(chave, { volta: chave });
                for (let i = 0; i < 20000; i++) {
                  const k = {}; wm.set(k, { k });                      // valor aponta a chave (efêmero)
                  const a = {}; const b = { a }; a.b = b;               // ciclo simples
                  const fn = function () { return fn; };              // função que se vê pelo ambiente
                  const p = new Promise(r => { a.r = r; }); p.then(() => a); // promessa ↔ objeto
                }"#,
                "gc",
            )
            .ok();
            let before = crate::gc::tracked();
            let st = crate::gc::collect();
            let after = crate::gc::tracked();
            e.eval("console.log(keep.self.self.name, cls(), cls.again(), keep.m.get(keep) === cls, wm.get(chave).volta === chave)", "depois").ok();
            assert!(before - base > 80_000, "antes {before} base {base}");
            assert!(after - base < 2_000, "sobrou {} (liberou {})", after - base, st.freed);
            let l = lines.borrow().join("\n");
            l
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(out, "vivo 1 2 true true");
}
