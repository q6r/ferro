// Emissor de eventos, observadores e um pequeno "store" reativo.
class Emissor {
  #ouvintes = new Map();
  on(evento, fn) {
    if (!this.#ouvintes.has(evento)) this.#ouvintes.set(evento, []);
    this.#ouvintes.get(evento).push({ fn, uma: false });
    return () => this.off(evento, fn);
  }
  once(evento, fn) { this.on(evento, fn); this.#ouvintes.get(evento).at(-1).uma = true; return this; }
  off(evento, fn) {
    const lista = this.#ouvintes.get(evento);
    if (!lista) return false;
    const i = lista.findIndex(o => o.fn === fn);
    if (i >= 0) lista.splice(i, 1);
    return i >= 0;
  }
  emit(evento, ...args) {
    const lista = this.#ouvintes.get(evento);
    if (!lista || lista.length === 0) {
      if (evento === "erro") throw args[0] instanceof Error ? args[0] : new Error(String(args[0]));
      return false;
    }
    for (const o of [...lista]) {
      if (o.uma) this.off(evento, o.fn);
      o.fn.apply(this, args);
    }
    return true;
  }
  contar(evento) { return this.#ouvintes.get(evento)?.length ?? 0; }
}

const log = [];
const em = new Emissor();
const tira = em.on("dado", (x, y) => log.push(`a:${x}+${y}`));
em.on("dado", function (x) { log.push(`b:${x}:${this instanceof Emissor}`); });
em.once("dado", x => log.push(`uma vez:${x}`));
em.emit("dado", 1, 2);
em.emit("dado", 3, 4);
tira();
em.emit("dado", 5);
console.log(log, em.contar("dado"), em.emit("nada"));
try { em.emit("erro", "falha grave"); } catch (e) { console.log("capturado:", e.message); }

function criarStore(reducer, inicial) {
  let estado = inicial;
  const inscritos = new Set();
  return {
    get estado() { return estado; },
    despachar(acao) {
      const anterior = estado;
      estado = reducer(estado, acao);
      if (estado !== anterior) inscritos.forEach(f => f(estado, anterior));
      return acao;
    },
    inscrever(f) { inscritos.add(f); return () => inscritos.delete(f); },
  };
}
