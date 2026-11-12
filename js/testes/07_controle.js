function classifica(n) {
  switch (true) { case n < 0: return "negativo"; case n === 0: return "zero"; case n < 10: return "pequeno"; default: return "grande"; }
}
console.log([-5, 0, 3, 50].map(classifica));
function fallthrough(x) { const r = []; switch (x) { case 1: r.push(1); case 2: r.push(2); break; case 3: { let y = 3; r.push(y); } default: r.push("d"); } return r; }
console.log(fallthrough(1), fallthrough(2), fallthrough(3), fallthrough(9));
const log = [];
for (let i = 0, j = 10; i < j; i += 3, j -= 2) log.push(`${i}:${j}`);
console.log(log.join(" "));
let it = 0; while (it < 100) { it++; if (it % 2) continue; if (it > 10) break; } console.log(it);
function tf() { const r = []; try { r.push("try"); throw 1; } catch (e) { r.push("catch" + e); return r; } finally { r.push("finally"); } }
console.log(tf());
function tf2() { try { return "a"; } finally { return "b"; } }
function tf3() { for (let i = 0; i < 3; i++) { try { continue; } finally { if (i == 1) return "f" + i; } } }
console.log(tf2(), tf3());
const nested = []; a: for (let i = 0; i < 3; i++) { b: for (let j = 0; j < 3; j++) { if (j == 1) continue a; if (i == 2) break a; nested.push([i, j]); } }
console.log(nested);
try { try { throw new Error("interno"); } finally { console.log("limpa"); } } catch (e) { console.log("pegou", e.message); }
try { null.f(); } catch { console.log("catch sem parâmetro"); }
const pares = []; for (const [k, v] of Object.entries({ a: 1, b: 2 })) pares.push(k + v); console.log(pares);
const it2 = { [Symbol.iterator]() { let i = 0; return { next() { return { value: i, done: i++ >= 3 }; }, return() { console.log("fechou"); return {}; } }; } };
for (const x of it2) { if (x === 1) break; }
const [primeiro] = it2; console.log(primeiro);
let soma = 0; for (const n of new Set([1, 2, 2, 3])) soma += n; for (const ch of "ab") soma += ch.length; console.log(soma);
const objIn = { x: 1, y: 2 }; Object.defineProperty(objIn, "z", { value: 3, enumerable: false }); const ks = []; for (const k in objIn) ks.push(k); console.log(ks);
const arrIn = [7, 8]; arrIn.extra = 1; const ai = []; for (const k in arrIn) ai.push(k); console.log(ai);
console.log((() => { if (true) { function inner() { return "annex b"; } } return inner(); })());
do { var dv = "do-while"; } while (false); console.log(dv);
console.log([1, 2, 3].map(x => { switch (x) { case 1: return "um"; default: return "?"; } }));
function exc() { try { throw new Error("x"); } catch (e) { return "c"; } finally { console.log("fin exc"); } }
console.log(exc());
const r = []; for (const x of [1, 2, 3]) { try { if (x === 2) throw x; r.push(x); } catch (e) { r.push("e" + e); continue; } finally { r.push("f"); } }
console.log(r);
const herd = Object.create({ herdada: 1 }); herd.propria = 2; const hk = []; for (const k in herd) hk.push(k); console.log(hk);
let w = 0; outer: while (true) { while (true) { w++; if (w > 3) break outer; continue outer; } } console.log(w);
label1: { console.log("antes"); break label1; console.log("nunca"); } console.log("depois");
const vals = []; for (let i = 0; i < 3; i++) { vals.push(i); } console.log(vals.length);
