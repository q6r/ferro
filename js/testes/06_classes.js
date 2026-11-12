class Forma {
  static contador = 0;
  #id;
  constructor(nome) { this.nome = nome; this.#id = ++Forma.contador; }
  get id() { return this.#id; }
  area() { return 0; }
  descricao() { return `${this.nome} #${this.#id} área=${this.area().toFixed(2)}`; }
  toString() { return `[Forma ${this.nome}]`; }
  static comparar(a, b) { return a.area() - b.area(); }
}
class Circulo extends Forma {
  constructor(r) { super("círculo"); this.r = r; }
  area() { return Math.PI * this.r ** 2; }
}
class Quadrado extends Forma {
  lado = 1;
  constructor(l) { super("quadrado"); this.lado = l; }
  area() { return this.lado ** 2; }
  descricao() { return super.descricao() + " (quadrado)"; }
}
const formas = [new Circulo(1), new Quadrado(3), new Circulo(0.5)];
console.log(formas.sort(Forma.comparar).map(f => f.descricao()));
console.log(formas[0], `${formas[1]}`, formas[1] instanceof Forma, Forma.contador, formas[2].id);
console.log(Object.getOwnPropertyNames(Forma.prototype), Object.keys(formas[1]), typeof Forma, Forma.name, Circulo.length);
class Pilha extends Array { topo() { return this[this.length - 1]; } }
const p = new Pilha(); p.push(1, 2, 3); console.log(p.topo(), p.length, p instanceof Array, Array.isArray(p));
class MeuErro extends Error { constructor(msg, codigo) { super(msg); this.name = "MeuErro"; this.codigo = codigo; } }
try { throw new MeuErro("deu ruim", 42); } catch (e) { console.log(e.name, e.message, e.codigo, e instanceof MeuErro, e instanceof Error, `${e}`); }
const Mixin = Base => class extends Base { mix() { return "mix " + this.nome; } };
class Misturado extends Mixin(Forma) {}
console.log(new Misturado("m").mix(), Object.getPrototypeOf(Misturado.prototype) !== Forma.prototype);
class Contador { static #n = 0; static inc() { return ++Contador.#n; } #priv() { return 1; } static has(o) { return #priv in o; } }
console.log(Contador.inc(), Contador.inc(), Contador.has(new Contador()), Contador.has({}));
class Acessores { #v = 1; get v() { return this.#v; } set v(x) { this.#v = x * 10; } static get s() { return "estático"; } }
const ac = new Acessores(); ac.v = 2; console.log(ac.v, Acessores.s, ac);
class Campos { a = 1; b = this.a + 1; ["c" + 1] = 3; static s = Campos.name + "!"; arrow = () => this.a; }
const cf = new Campos(); console.log(cf.a, cf.b, cf.c1, Campos.s, cf.arrow.call(null));
const Anon = class {}; const Named = class Interno { quem() { return Interno.name; } };
console.log(Anon.name, Named.name, new Named().quem());
console.log(Object.getPrototypeOf(class extends Forma {}) === Forma);
class Base { constructor() { this.tipo = new.target.name; } quem() { return "base"; } static criar() { return new this(); } }
class Filha extends Base { quem() { return "filha>" + super.quem(); } }
console.log(new Filha().tipo, Filha.criar().quem(), new Base().quem());
class ComGetterHerdado { get x() { return 1; } } class Sub extends ComGetterHerdado { get x() { return super.x + 1; } }
console.log(new Sub().x);
class Estatico { static a = 1; static b = this.a + 1; static { this.c = this.b * 10; } }
console.log(Estatico.a, Estatico.b, Estatico.c);
class Pessoa { constructor(n) { this.n = n; } } const ps = [new Pessoa("a"), new Pessoa("b")]; console.log(ps);
