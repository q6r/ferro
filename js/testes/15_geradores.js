function* contar(n) { for (let i = 1; i <= n; i++) yield i; return "fim"; }
const g = contar(3);
console.log(g.next(), g.next(), g.next(), g.next(), g.next());
console.log([...contar(5)], Array.from(contar(2)), Math.max(...contar(4)));
function* eco() { let recebido = "início"; while (true) { recebido = yield recebido; if (recebido === "parar") return "parou"; } }
const e = eco(); console.log(e.next("ignorado"), e.next("a"), e.next("b"), e.next("parar"), e.next("x"));
function* comFinally() { try { yield 1; yield 2; } finally { console.log("limpeza"); } }
for (const x of comFinally()) { console.log("valor", x); break; }
const cf = comFinally(); cf.next(); console.log(cf.return(42), cf.next());
function* capturaErro() { while (true) { try { yield "ok"; } catch (err) { console.log("pegou", err); yield "recuperado"; } } }
const ce = capturaErro(); console.log(ce.next().value, ce.throw("boom").value, ce.next().value);
const naoIniciado = contar(3); try { naoIniciado.throw(new Error("antes")); } catch (x) { console.log("erro direto:", x.message); } console.log(naoIniciado.next());
function* delegado() { const r = yield* contar(2); console.log("retorno do delegado:", r); yield* ["a", "b"]; yield* "xy"; }
console.log([...delegado()]);
function* fib() { let [a, b] = [0, 1]; for (;;) { yield a; [a, b] = [b, a + b]; } }
const primeiros = []; for (const n of fib()) { if (n > 100) break; primeiros.push(n); } console.log(primeiros.join(" "));
function* take(it, n) { let i = 0; for (const x of it) { if (i++ >= n) return; yield x; } }
function* map(it, f) { for (const x of it) yield f(x); }
function* filter(it, p) { for (const x of it) if (p(x)) yield x; }
console.log([...take(filter(map(fib(), x => x * 3), x => x % 2 === 0), 6)]);
const obj = { *[Symbol.iterator]() { yield* Object.entries(this); }, a: 1, b: 2 };
console.log([...obj]);
class Arvore {
  constructor(v, ...filhos) { this.v = v; this.filhos = filhos; }
  *[Symbol.iterator]() { yield this.v; for (const f of this.filhos) yield* f; }
}
const arv = new Arvore(1, new Arvore(2, new Arvore(4)), new Arvore(3));
console.log([...arv], Object.prototype.toString.call(contar(1)), typeof contar(1).next, contar(1)[Symbol.iterator]() instanceof contar);
const gen = contar(2); console.log(gen, String(gen));
function* rodando() { try { gen2.next(); } catch (x) { yield x.message; } } const gen2 = rodando(); console.log(gen2.next().value);
function* args() { yield arguments.length; yield this && this.tag; } console.log([...args.call({ tag: "T" }, 1, 2, 3)]);
const anon = function* () { yield* [1, 2]; }; console.log(anon.name, [...anon()]);
function* retornoComFinally() { try { yield 1; } finally { yield "do finally"; } }
const rf = retornoComFinally(); rf.next(); console.log(rf.return("x"), rf.next(), rf.next());
const [p1, p2, ...resto] = contar(6); console.log(p1, p2, resto);
function* comDefault(a = 10, { b } = { b: 20 }) { yield a + b; } console.log([...comDefault()], [...comDefault(1, { b: 2 })]);
