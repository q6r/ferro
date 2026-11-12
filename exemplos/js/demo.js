// Demonstração do Ferro JS: o motor de JavaScript próprio do Ferro.
class Conta {
  #saldo = 0;
  static #contas = 0;
  constructor(titular) {
    this.titular = titular;
    Conta.#contas++;
  }
  depositar(valor) {
    if (valor <= 0) throw new RangeError(`valor inválido: ${valor}`);
    this.#saldo += valor;
    return this;
  }
  get saldo() { return this.#saldo; }
  static get total() { return Conta.#contas; }
}

const contas = ["Ana", "Bruno", "Carla"].map(n => new Conta(n));
contas[0].depositar(100).depositar(50);
contas[1].depositar(30);
try { contas[2].depositar(-5); } catch (e) { console.log(`${e.name}: ${e.message}`); }

const resumo = Object.fromEntries(contas.map(({ titular, saldo }) => [titular, saldo]));
console.log(resumo, "contas:", Conta.total);

const porFaixa = Object.groupBy(contas, c => (c.saldo >= 50 ? "alto" : "baixo"));
console.log(Object.keys(porFaixa).sort(), porFaixa.alto.map(c => c.titular));

const m = new Map(contas.map(c => [c.titular, c.saldo]));
console.log(m, [...m.values()].reduce((a, b) => a + b, 0));

const fib = n => (n < 2 ? n : fib(n - 1) + fib(n - 2));
console.time("fib(24)");
console.log("fib(24) =", fib(24));
console.timeEnd("fib(24)");

console.log(JSON.stringify({ data: new Date(Date.UTC(2026, 10, 11)), lista: [1, 2, 3] }));
console.log([3, 1, 2].toSorted(), "ferro".split("").reverse().join(""), 0.1 + 0.2 === 0.3);
