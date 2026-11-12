// Medidas simples de desempenho do interpretador.
function medir(nome, fn) {
  const t = Date.now();
  const r = fn();
  console.log(nome.padEnd(22), String(Date.now() - t).padStart(6), "ms", r);
}
medir("fib(27) recursivo", () => { const fib = n => n < 2 ? n : fib(n - 1) + fib(n - 2); return fib(27); });
medir("laço 3M somas", () => { let s = 0; for (let i = 0; i < 3e6; i++) s += i % 7; return s; });
medir("array 300k push/map", () => { const a = []; for (let i = 0; i < 3e5; i++) a.push(i); return a.map(x => x * 2).filter(x => x % 3 == 0).length; });
medir("objetos 200k", () => { let t = 0; for (let i = 0; i < 2e5; i++) { const o = { x: i, y: i * 2 }; t += o.x + o.y; } return t; });
medir("métodos de classe", () => { class P { constructor(x) { this.x = x; } dobro() { return this.x * 2; } } let s = 0; for (let i = 0; i < 2e5; i++) s += new P(i).dobro(); return s; });
medir("strings 100k concat", () => { let s = ""; for (let i = 0; i < 1e5; i++) s += "ab"; return s.length; });
medir("Map 100k", () => { const m = new Map(); for (let i = 0; i < 1e5; i++) m.set("k" + i, i); let s = 0; for (const [, v] of m) s += v; return s; });
medir("JSON ida e volta", () => { const o = Array.from({ length: 5000 }, (_, i) => ({ id: i, nome: "item" + i, tags: ["a", "b"] })); return JSON.parse(JSON.stringify(o)).length; });
medir("sort 100k", () => { let seed = 1; const a = Array.from({ length: 1e5 }, () => (seed = (seed * 16807) % 2147483647)); a.sort((x, y) => x - y); return a[0] <= a[1]; });
const texto = Array.from({ length: 20000 }, (_, i) => `linha ${i}: usuario${i % 97}@exemplo.com visitou /pagina/${i % 13}?x=${i}`).join("\n");
medir("regex replace global", () => texto.replace(/\d+/g, "#").length);
medir("regex matchAll emails", () => [...texto.matchAll(/(\w+)@(\w+)\.com/g)].length);
medir("regex split linhas", () => texto.split(/\r?\n/).length);
medir("regex test em laço", () => { const r = /^linha \d+: usuario(\d+)@/; let n = 0; for (const l of texto.split("\n")) if (r.test(l)) n++; return n; });
medir("regex .* com lookahead", () => (texto.match(/^(?=.*pagina\/7).*$/gm) || []).length);
