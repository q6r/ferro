const dorme = (ms, v) => new Promise(r => setTimeout(() => r(v), ms));
async function* contagem(n) { for (let i = 1; i <= n; i++) { await dorme(2); yield i; } return "acabou"; }
(async () => {
  const valores = [];
  for await (const v of contagem(3)) valores.push(v);
  console.log("for await", valores);
  const g = contagem(2);
  console.log(await g.next(), await g.next(), await g.next(), await g.next());
  for await (const x of [Promise.resolve("a"), "b", dorme(5, "c")]) console.log("sync iterável", x);
  async function* paginas() { let pagina = 0; while (pagina < 3) { const itens = await dorme(1, [pagina * 2, pagina * 2 + 1]); yield itens; pagina++; } }
  const todos = []; for await (const p of paginas()) todos.push(...p); console.log("paginado", todos);
  const fila = contagem(5); const promessas = [fila.next(), fila.next(), fila.next()];
  console.log("fila", (await Promise.all(promessas)).map(r => r.value));
  console.log("return", await fila.return("cedo"), await fila.next());
  async function* comErro() { yield 1; throw new Error("no gerador async"); }
  try { for await (const x of comErro()) console.log("antes do erro", x); } catch (e) { console.log("capturou", e.message); }
  async function* limpa() { try { yield 1; yield 2; } finally { console.log("finally async gen"); } }
  for await (const x of limpa()) { console.log("primeiro", x); break; }
  const lazy = contagem(1); console.log(Object.prototype.toString.call(lazy), typeof lazy[Symbol.asyncIterator]);
  const comThrow = contagem(2); await comThrow.next(); try { await comThrow.throw(new Error("de fora")); } catch (e) { console.log("throw de fora:", e.message); }
  console.log(await comThrow.next());
  const it = { [Symbol.asyncIterator]() { let i = 0; return { next: async () => ({ value: i, done: i++ >= 2 }) }; } };
  for await (const v of it) console.log("iterador manual", v);
  console.log("fim");
})();
