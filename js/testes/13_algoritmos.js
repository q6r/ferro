// Algoritmos clássicos.
function quicksort(a) {
  if (a.length <= 1) return a;
  const [p, ...resto] = a;
  return [...quicksort(resto.filter(x => x < p)), p, ...quicksort(resto.filter(x => x >= p))];
}
function mergesort(a, cmp = (x, y) => x - y) {
  if (a.length < 2) return a.slice();
  const m = a.length >> 1, e = mergesort(a.slice(0, m), cmp), d = mergesort(a.slice(m), cmp);
  const out = [];
  while (e.length && d.length) out.push(cmp(e[0], d[0]) <= 0 ? e.shift() : d.shift());
  return out.concat(e, d);
}
let seed = 42;
const rand = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
const dados = Array.from({ length: 40 }, () => Math.floor(rand() * 100));
console.log(dados.join(" "));
console.log(quicksort(dados).join(" "));
console.log(JSON.stringify(mergesort(dados)) === JSON.stringify([...dados].sort((a, b) => a - b)));
function busca(a, x) { let lo = 0, hi = a.length - 1; while (lo <= hi) { const m = (lo + hi) >>> 1; if (a[m] === x) return m; a[m] < x ? (lo = m + 1) : (hi = m - 1); } return -1; }
const ord = mergesort(dados);
console.log(busca(ord, ord[17]) >= 0, busca(ord, 1000));
const grafo = { A: ["B", "C"], B: ["D"], C: ["D", "E"], D: ["F"], E: ["F"], F: [] };
function bfs(g, ini, fim) {
  const fila = [[ini]], vistos = new Set([ini]);
  while (fila.length) {
    const caminho = fila.shift(), no = caminho.at(-1);
    if (no === fim) return caminho;
    for (const v of g[no]) if (!vistos.has(v)) { vistos.add(v); fila.push([...caminho, v]); }
  }
  return null;
}
console.log(bfs(grafo, "A", "F"), bfs(grafo, "F", "A"));
function topo(g) { const vis = new Set(), out = []; const dfs = n => { if (vis.has(n)) return; vis.add(n); g[n].forEach(dfs); out.unshift(n); }; Object.keys(g).forEach(dfs); return out; }
console.log(topo(grafo).join(" > "));
function lcs(a, b) {
  const dp = Array.from({ length: a.length + 1 }, () => new Array(b.length + 1).fill(0));
  for (let i = 1; i <= a.length; i++) for (let j = 1; j <= b.length; j++)
    dp[i][j] = a[i - 1] === b[j - 1] ? dp[i - 1][j - 1] + 1 : Math.max(dp[i - 1][j], dp[i][j - 1]);
  let i = a.length, j = b.length, r = "";
  while (i && j) { if (a[i - 1] === b[j - 1]) { r = a[i - 1] + r; i--; j--; } else if (dp[i - 1][j] >= dp[i][j - 1]) i--; else j--; }
  return r;
}
console.log(lcs("navegador", "nave espacial"), lcs("ABCBDAB", "BDCABA"));
const palavras = ["roma", "amor", "mora", "ramo", "pedra", "padre", "perda", "sol"];
const grupos = palavras.reduce((m, p) => { const k = [...p].sort().join(""); (m[k] ||= []).push(p); return m; }, {});
console.log(Object.values(grupos));
const ehPalindromo = s => { const t = s.toLowerCase().normalize("NFD").replace(/[^a-z]/g, ""); return t === [...t].reverse().join(""); };
try { console.log(ehPalindromo("A base do teto desaba")); } catch (e) { console.log("regex:", e.constructor.name); }
const mat = (a, b) => a.map(l => b[0].map((_, j) => l.reduce((s, v, k) => s + v * b[k][j], 0)));
console.log(mat([[1, 2], [3, 4]], [[5, 6], [7, 8]]));
const primos = n => { const crivo = new Array(n + 1).fill(true); crivo[0] = crivo[1] = false; for (let i = 2; i * i <= n; i++) if (crivo[i]) for (let j = i * i; j <= n; j += i) crivo[j] = false; return crivo.reduce((acc, p, i) => (p && acc.push(i), acc), []); };
console.log(primos(60));
const fibMemo = (() => { const c = [0, 1]; return function f(n) { return c[n] ?? (c[n] = f(n - 1) + f(n - 2)); }; })();
console.log(fibMemo(90), fibMemo(50) % 1000);
const hanoi = (n, a = "A", b = "B", c = "C") => n === 0 ? [] : [...hanoi(n - 1, a, c, b), `${a}->${c}`, ...hanoi(n - 1, b, a, c)];
console.log(hanoi(3).join(" "), hanoi(10).length);
const perm = xs => xs.length <= 1 ? [xs] : xs.flatMap((x, i) => perm([...xs.slice(0, i), ...xs.slice(i + 1)]).map(p => [x, ...p]));
console.log(perm([1, 2, 3]).map(p => p.join("")).join(","));
const rle = s => [...s].reduce((acc, c) => { const last = acc.at(-1); last && last[0] === c ? last[1]++ : acc.push([c, 1]); return acc; }, []).map(([c, n]) => n > 1 ? n + c : c).join("");
console.log(rle("aaabccddddde"));
const bigSum = Array.from({ length: 10000 }, (_, i) => i).reduce((a, b) => a + b);
console.log(bigSum, Math.max(...Array.from({ length: 1000 }, (_, i) => (i * 7919) % 1009)));
