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

