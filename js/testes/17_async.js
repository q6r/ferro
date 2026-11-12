const dorme = (ms, v) => new Promise(r => setTimeout(() => r(v), ms));
async function simples() { return 42; }
console.log(simples() instanceof Promise, simples.constructor === Function ? "Function" : "outro");
simples().then(v => console.log("simples", v));
async function sequencia() {
  const a = await dorme(40, "a");
  const b = await "b";
  const c = await Promise.resolve("c");
  return a + b + c;
}
sequencia().then(v => console.log("sequência", v));
async function falha() { await null; throw new RangeError("dentro do async"); }
falha().catch(e => console.log("capturado", e.name, e.message));
async function tenta() { try { await Promise.reject(new Error("rejeitada")); } catch (e) { return "tratou " + e.message; } finally { console.log("finally do async"); } }
tenta().then(console.log);
const ordem = [];
async function A() { ordem.push("A1"); await undefined; ordem.push("A2"); await null; ordem.push("A3"); }
async function B() { ordem.push("B1"); await undefined; ordem.push("B2"); }
A(); B(); ordem.push("sync");
Promise.resolve().then(() => ordem.push("p1")).then(() => ordem.push("p2")).then(() => ordem.push("p3"));
setTimeout(() => console.log("ordem", ordem.join(" ")), 0);
const arrow = async (x) => (await dorme(1, x)) * 2; arrow(21).then(v => console.log("arrow", v));
class Servico { constructor() { this.n = 0; } async inc() { await null; return ++this.n; } static async criar() { return new Servico(); } }
Servico.criar().then(async s => { await s.inc(); await s.inc(); console.log("método async", await s.inc()); });
async function paralelo() { const t = Date.now(); const r = await Promise.all([dorme(80, 1), dorme(80, 2), dorme(80, 3)]); return [r, Date.now() - t < 200]; }
paralelo().then(r => console.log("paralelo", r));
async function* nada() {}
console.log(typeof nada);
async function laco() { let s = 0; for (const x of [1, 2, 3]) s += await x; for (let i = 0; i < 3; i++) s += await dorme(1, i); return s; }
laco().then(v => console.log("laço com await", v));
async function aninhada() { const f = async () => { await null; return this?.nome ?? "sem this"; }; return f(); }
aninhada.call({ nome: "obj" }).then(v => console.log("this na arrow async", v));
async function semRetorno() {} semRetorno().then(v => console.log("sem retorno", v));
const objAsync = { async metodo() { return "obj"; }, async *gen() {} };
objAsync.metodo().then(v => console.log("método literal", v));
async function awaitThenable() { return await { then(r) { r("thenable"); } }; } awaitThenable().then(v => console.log(v));
async function lancaSync(x) { if (!x) throw new Error("síncrono"); return x; }
lancaSync(0).catch(e => console.log("vira rejeição:", e.message));
(async () => { const r = []; for (const p of [dorme(3, "x"), dorme(1, "y")]) r.push(await p); console.log("for com await", r); })();
