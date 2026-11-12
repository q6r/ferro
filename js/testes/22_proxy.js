const log = [];
const alvo = { nome: "Ana", idade: 30 };
const p = new Proxy(alvo, {
  get(t, k, r) { log.push(`get ${String(k)}`); return Reflect.get(t, k, r); },
  set(t, k, v) { log.push(`set ${k}=${v}`); t[k] = typeof v === "string" ? v.toUpperCase() : v; return true; },
  has(t, k) { return k.startsWith("_") ? false : k in t; },
  deleteProperty(t, k) { log.push(`delete ${k}`); return delete t[k]; },
  ownKeys(t) { return [...Reflect.ownKeys(t), "virtual"]; },
  getOwnPropertyDescriptor(t, k) { return k === "virtual" ? { value: "v", enumerable: true, configurable: true } : Reflect.getOwnPropertyDescriptor(t, k); },
});
p.cidade = "rio"; console.log(p.nome, p.cidade, alvo.cidade, "nome" in p, "_x" in p, delete p.idade, Object.keys(p), log);
console.log(JSON.stringify(p), { ...p }, Object.entries(p).length);
function reativo(obj, efeito) {
  return new Proxy(obj, { set(t, k, v) { const antigo = t[k]; t[k] = v; if (antigo !== v) efeito(k, v, antigo); return true; } });
}
const mudancas = []; const estado = reativo({ contador: 0, itens: [] }, (k, v, a) => mudancas.push(`${k}: ${JSON.stringify(a)} -> ${JSON.stringify(v)}`));
estado.contador++; estado.contador++; estado.contador = 2; estado.itens = [1]; console.log(mudancas);
const fn = new Proxy(function soma(a, b) { return a + b; }, { apply(t, self, args) { return t(...args) * 10; } });
console.log(fn(1, 2), typeof fn, fn.name, fn.length);
class Base { constructor(x) { this.x = x; } }
const C = new Proxy(Base, { construct(t, args) { return new t(args[0] * 2); } });
console.log(new C(21).x, new C(1) instanceof Base);
const arr = new Proxy([1, 2, 3], { get(t, k) { return k === "length" ? t.length : (typeof k === "string" && /^\d+$/.test(k) ? t[k] * 100 : t[k]); } });
console.log(Array.isArray(arr), arr[1], arr.length, arr.map(x => x + 1), [...arr].length);
const { proxy, revoke } = Proxy.revocable({ a: 1 }, {}); console.log(proxy.a); revoke();
try { proxy.a; } catch (e) { console.log(e.name, e.message); }
const semTrap = new Proxy({ v: 1 }, {}); semTrap.w = 2; console.log(semTrap.v, semTrap.w, Object.keys(semTrap), semTrap);
const defaults = new Proxy({}, { get: (t, k) => k in t ? t[k] : `padrão(${String(k)})` }); defaults.x = 1; console.log(defaults.x, defaults.y);
const proto = new Proxy({}, { get: (t, k, r) => k === "herdado" ? "via proxy no protótipo" : undefined }); const filho = Object.create(proto); console.log(filho.herdado);
const validado = new Proxy({}, { set(t, k, v) { if (typeof v !== "number") throw new TypeError(`${k} precisa ser número`); t[k] = v; return true; } });
try { validado.n = 1; validado.s = "x"; } catch (e) { console.log(e.message, validado.n); }
const traps = []; const rastreado = new Proxy({ a: 1 }, new Proxy({}, { get(_, trap) { traps.push(trap); return undefined; } }));
rastreado.a; rastreado.b = 2; "a" in rastreado; Object.keys(rastreado); delete rastreado.a; Object.getPrototypeOf(rastreado);
console.log(traps);
console.log(Reflect.ownKeys(new Proxy({ z: 1, [Symbol("s")]: 2 }, {})).length, Object.getPrototypeOf(new Proxy([], {})) === Array.prototype);
