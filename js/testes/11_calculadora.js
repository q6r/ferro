// Calculadora: tokenizador, parser descendente recursivo e avaliador.
class Token {
  constructor(tipo, valor, pos) { Object.assign(this, { tipo, valor, pos }); }
}

function tokenizar(src) {
  const toks = [];
  let i = 0;
  const isDigit = c => c >= "0" && c <= "9";
  const isAlpha = c => (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || c === "_";
  while (i < src.length) {
    const c = src[i];
    if (c === " ") { i++; continue; }
    if (isDigit(c) || (c === "." && isDigit(src[i + 1]))) {
      let j = i;
      while (j < src.length && (isDigit(src[j]) || src[j] === ".")) j++;
      toks.push(new Token("num", parseFloat(src.slice(i, j)), i));
      i = j;
    } else if (isAlpha(c)) {
      let j = i;
      while (j < src.length && (isAlpha(src[j]) || isDigit(src[j]))) j++;
      toks.push(new Token("id", src.slice(i, j), i));
      i = j;
    } else if ("+-*/^(),=".includes(c)) {
      toks.push(new Token("op", c, i++));
    } else {
      throw new SyntaxError(`caractere inesperado '${c}' na posição ${i}`);
    }
  }
  toks.push(new Token("fim", null, i));
  return toks;
}

class Parser {
  #toks; #i = 0;
  constructor(toks) { this.#toks = toks; }
  get atual() { return this.#toks[this.#i]; }
  #come(tipo, valor) {
    const t = this.atual;
    if (t.tipo !== tipo || (valor !== undefined && t.valor !== valor)) throw new SyntaxError(`esperava ${valor ?? tipo} em ${t.pos}, veio ${t.valor ?? t.tipo}`);
    this.#i++;
    return t;
  }
  programa() {
    if (this.atual.tipo === "id" && this.#toks[this.#i + 1]?.valor === "=") {
      const nome = this.#come("id").valor; this.#come("op", "=");
      return { tipo: "atrib", nome, valor: this.expr() };
    }
    const e = this.expr(); this.#come("fim"); return e;
  }
  expr() {
    let esq = this.termo();
    while (this.atual.valor === "+" || this.atual.valor === "-") {
      const op = this.#come("op").valor;
      esq = { tipo: "bin", op, esq, dir: this.termo() };
    }
    return esq;
  }
  termo() {
    let esq = this.fator();
    while (["*", "/"].includes(this.atual.valor)) {
      const op = this.#come("op").valor;
      esq = { tipo: "bin", op, esq, dir: this.fator() };
    }
    return esq;
  }
  fator() {
    const base = this.unario();
    if (this.atual.valor === "^") { this.#come("op"); return { tipo: "bin", op: "^", esq: base, dir: this.fator() }; }
    return base;
  }
  unario() {
    if (this.atual.valor === "-") { this.#come("op"); return { tipo: "neg", arg: this.unario() }; }
    return this.primario();
  }
  primario() {
    const t = this.atual;
    if (t.tipo === "num") { this.#i++; return { tipo: "num", v: t.valor }; }
    if (t.tipo === "id") {
      this.#i++;
      if (this.atual.valor === "(") {
        this.#come("op", "(");
        const args = [];
        if (this.atual.valor !== ")") { do { args.push(this.expr()); } while (this.atual.valor === "," && this.#come("op", ",")); }
        this.#come("op", ")");
        return { tipo: "chamada", nome: t.valor, args };
      }
      return { tipo: "var", nome: t.valor };
    }
    if (t.valor === "(") { this.#come("op", "("); const e = this.expr(); this.#come("op", ")"); return e; }
    throw new SyntaxError(`token inesperado ${t.valor ?? t.tipo} em ${t.pos}`);
  }
}

const ambiente = new Map(Object.entries({ pi: Math.PI, e: Math.E }));
const funcoes = { max: Math.max, min: Math.min, sqrt: Math.sqrt, abs: Math.abs, soma: (...xs) => xs.reduce((a, b) => a + b, 0) };
function avaliar(no) {
  switch (no.tipo) {
    case "num": return no.v;
    case "var": if (!ambiente.has(no.nome)) throw new ReferenceError(`${no.nome} não definida`); return ambiente.get(no.nome);
    case "neg": return -avaliar(no.arg);
    case "atrib": { const v = avaliar(no.valor); ambiente.set(no.nome, v); return v; }
    case "chamada": {
      const f = funcoes[no.nome];
      if (typeof f !== "function") throw new TypeError(`${no.nome} não é função`);
      return f(...no.args.map(avaliar));
    }
    case "bin": {
      const a = avaliar(no.esq), b = avaliar(no.dir);
      return { "+": a + b, "-": a - b, "*": a * b, "/": a / b, "^": a ** b }[no.op];
    }
  }
}
const linhas = ["1 + 2 * 3", "(1 + 2) * 3", "2 ^ 3 ^ 2", "-2 ^ 2", "x = 10", "x * pi", "max(1, x, 3) + soma(1, 2, 3, 4)",
  "sqrt(16) / 0", "y + 1", "1 + ", "3 $ 4", "abs(-7.5)", "soma()", "nada(1)"];
for (const l of linhas) {
  try { console.log(l.padEnd(32), "=>", avaliar(new Parser(tokenizar(l)).programa())); }
  catch (e) { console.log(l.padEnd(32), "!!", `${e.name}: ${e.message}`); }
}
console.log(JSON.stringify(new Parser(tokenizar("1 + f(2, 3)")).programa()));
console.log(new Parser(tokenizar("a * (b - 1)")).programa());
