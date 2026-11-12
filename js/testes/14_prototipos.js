// Protótipos, coerção e protocolos.
function Animal(nome) { this.nome = nome; }
Animal.prototype.falar = function () { return `${this.nome} faz um som`; };
Animal.prototype.toString = function () { return `Animal(${this.nome})`; };
function Gato(nome) { Animal.call(this, nome); this.vidas = 7; }
Gato.prototype = Object.create(Animal.prototype);
Gato.prototype.constructor = Gato;
Gato.prototype.falar = function () { return Animal.prototype.falar.call(this) + " (miau)"; };
const g = new Gato("Tom");
console.log(g.falar(), String(g), g instanceof Animal, g.constructor.name, Object.keys(g), g);
console.log(Object.getPrototypeOf(Object.getPrototypeOf(g)) === Animal.prototype, Gato.prototype.isPrototypeOf(g));
const dinheiro = { valor: 42.5, valueOf() { return this.valor; }, toString() { return `R$ ${this.valor.toFixed(2)}`; } };
console.log(dinheiro + 1, `${dinheiro}`, dinheiro * 2, String(dinheiro), dinheiro > 40, [dinheiro] + "");
const soToString = { toString() { return "7"; } };
console.log(soToString * 2, soToString + 1, +soToString, soToString == 7);
class Par { static [Symbol.hasInstance](n) { return n % 2 === 0; } }
console.log(2 instanceof Par, 3 instanceof Par);
class Temperatura {
  #c = 0;
  static deF(f) { const t = new Temperatura(); t.f = f; return t; }
  get f() { return this.#c * 9 / 5 + 32; }
  set f(v) { this.#c = (v - 32) * 5 / 9; }
  get c() { return this.#c; }
  toJSON() { return { celsius: Math.round(this.#c * 10) / 10 }; }
  [Symbol.toPrimitive](hint) { return hint === "string" ? `${this.#c.toFixed(1)}°C` : this.#c; }
}
const t = Temperatura.deF(212);
console.log(t.c, t.f, `${t}`, t + 0, JSON.stringify({ t }), t > 50);
const base = { saudacao: "olá", cumprimentar() { return `${this.saudacao}, ${this.nome}`; } };
const pessoa = Object.setPrototypeOf({ nome: "Rui" }, base);
const outra = { __proto__: base, nome: "Ana", saudacao: "oi" };
console.log(pessoa.cumprimentar(), outra.cumprimentar(), Object.keys(outra), "saudacao" in pessoa);
const contador = Object.defineProperties({}, {
  _n: { value: 0, writable: true },
  n: { get() { return this._n; }, set(v) { this._n = Math.max(0, v); }, enumerable: true },
});
contador.n = 5; contador.n -= 10; console.log(contador.n, Object.keys(contador), JSON.stringify(contador));
const lista = { itens: ["a", "b", "c"], *[Symbol.iterator]() {} };
console.log(typeof lista[Symbol.iterator]);
const iteravel = { de: 3, [Symbol.iterator]() { let n = this.de; return { next: () => ({ done: n <= 0, value: n-- }) }; } };
console.log([...iteravel], Array.from(iteravel).map(x => x * x));
const proxyLike = new Map([["get", (o, k) => k in o ? o[k] : `<${String(k)}>`]]);
const alvo = { a: 1 }; console.log(proxyLike.get("get")(alvo, "a"), proxyLike.get("get")(alvo, "z"));
console.log(Object.prototype.toString.call(new Date(0)), Object.prototype.toString.call(/x/), Object.prototype.toString.call(function () {}));
console.log([1, 2, 3].toString === Array.prototype.toString, Array.prototype.slice.call("abc"), Array.prototype.map.call("ab", c => c + c));
console.log(Math.max.apply(null, [1, 5, 3]), Function.prototype.call.call(function () { return this; }, "x") instanceof String);
const arrLike = { 0: "x", 1: "y", length: 2 }; console.log(Array.from(arrLike), Array.prototype.join.call(arrLike, "+"));
console.log([..."abc"].entries().toString(), Object.entries([7, 8]));
