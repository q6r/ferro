// Prelúdio comum: a parte da plataforma que vale na página e nos Web
// Workers (eventos, DOMException, clone estruturado, crypto, texto, URL, Blob,
// fetch/XHR, portas de mensagem, Worker). O Rust junta este arquivo com o
// `prelude.js` (página) ou com o `worker.js` (worker), dentro da mesma
// função `(function (N) { ... })(__native)`.
//
// Cada variante define estas funções (declarações sobem para o topo da
// função, então podem vir depois): `baseURL()` (base dos endereços),
// `eventPath(t, s)` (caminho de um evento), `inlineHandlerOf(t, type)`
// (handler `on*` que não foi posto pela propriedade) e
// `formEntriesOf(form, submitter)` (o `new FormData(form)`).
const G = globalThis;
delete G.__native;

const hidden = (o, k, v) =>
  Object.defineProperty(o, k, { value: v, writable: true, configurable: true, enumerable: false });
const accessor = (o, k, get, set) =>
  Object.defineProperty(o, k, { get, set, configurable: true, enumerable: true });
const methods = (o, obj) => {
  for (const k of Reflect.ownKeys(obj)) hidden(o, k, obj[k]);
};
const named = (f, name) => Object.defineProperty(f, 'name', { value: name });

// ------------------------------------------------------------- global
const IS_WINDOW = typeof Window === 'function' && G instanceof Window;
hidden(G, 'isSecureContext', true);
hidden(G, 'origin', N.urlParse(N.href())?.origin ?? 'null');
hidden(G, 'reportError', (e) => N.reportError(e));
hidden(G, 'crossOriginIsolated', false);

// ---------------------------------------------------------- DOMException
const DOM_CODES = {
  IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5,
  NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InvalidStateError: 11,
  SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15,
  TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20, URLMismatchError: 21,
  QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25,
};
class DOMException extends Error {
  constructor(message = '', name = 'Error') {
    super(String(message));
    hidden(this, 'name', String(name));
  }
  get code() { return DOM_CODES[this.name] || 0; }
}
hidden(G, 'DOMException', DOMException);

// ------------------------------------------------------------- eventos
const EV = new WeakMap();
const LISTENERS = new WeakMap();
const HANDLERS = new WeakMap();
const INLINE = new WeakMap();

class Event {
  constructor(type, init) {
    if (arguments.length === 0) throw new TypeError("Failed to construct 'Event': 1 argument required, but only 0 present.");
    init = init || {};
    EV.set(this, {
      type: String(type), bubbles: !!init.bubbles, cancelable: !!init.cancelable, composed: !!init.composed,
      target: null, currentTarget: null, phase: 0, stop: false, stopNow: false, canceled: false,
      trusted: false, passive: false, dispatching: false, path: [], timeStamp: N.now(),
    });
    Object.defineProperty(this, 'isTrusted', { get: () => EV.get(this).trusted, enumerable: true });
  }
  get type() { return EV.get(this).type; }
  get target() { return EV.get(this).target; }
  get srcElement() { return EV.get(this).target; }
  get currentTarget() { return EV.get(this).currentTarget; }
  get eventPhase() { return EV.get(this).phase; }
  get bubbles() { return EV.get(this).bubbles; }
  get cancelable() { return EV.get(this).cancelable; }
  get composed() { return EV.get(this).composed; }
  get defaultPrevented() { return EV.get(this).canceled; }
  get timeStamp() { return EV.get(this).timeStamp; }
  get returnValue() { return !EV.get(this).canceled; }
  set returnValue(v) { if (!v) this.preventDefault(); }
  get cancelBubble() { return EV.get(this).stop; }
  set cancelBubble(v) { if (v) EV.get(this).stop = true; }
  preventDefault() { const s = EV.get(this); if (s.cancelable && !s.passive) s.canceled = true; }
  stopPropagation() { EV.get(this).stop = true; }
  stopImmediatePropagation() { const s = EV.get(this); s.stop = s.stopNow = true; }
  composedPath() { const s = EV.get(this); return s.dispatching ? s.path.slice() : []; }
  initEvent(type, bubbles, cancelable) {
    const s = EV.get(this);
    if (s.dispatching) return;
    s.type = String(type); s.bubbles = !!bubbles; s.cancelable = !!cancelable;
  }
}
for (const [k, v] of [['NONE', 0], ['CAPTURING_PHASE', 1], ['AT_TARGET', 2], ['BUBBLING_PHASE', 3]]) {
  Object.defineProperty(Event, k, { value: v });
  Object.defineProperty(Event.prototype, k, { value: v });
}
hidden(G, 'Event', Event);

/// Subclasses de Event com campos extras (lidos do dicionário `init`).
function eventClass(name, Parent, fields, extra) {
  const C = class extends Parent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      for (const k in fields) {
        Object.defineProperty(this, k, {
          value: k in init ? init[k] : fields[k], writable: true, enumerable: true, configurable: true,
        });
      }
      if (extra) extra(this, init);
    }
  };
  named(C, name);
  hidden(G, name, C);
  return C;
}
const CustomEvent = eventClass('CustomEvent', Event, { detail: null });
hidden(CustomEvent.prototype, 'initCustomEvent', function (type, b, c, detail) {
  this.initEvent(type, b, c); this.detail = detail;
});
const ErrorEvent = eventClass('ErrorEvent', Event, { message: '', filename: '', lineno: 0, colno: 0, error: null });
const MessageEvent = eventClass('MessageEvent', Event, { data: null, origin: '', lastEventId: '', source: null, ports: [] });
const ProgressEvent = eventClass('ProgressEvent', Event, { lengthComputable: false, loaded: 0, total: 0 });
eventClass('PromiseRejectionEvent', Event, { promise: null, reason: undefined });

const ETP = EventTarget.prototype;
function flagsOf(o) {
  if (typeof o === 'boolean') return { capture: o };
  if (o && typeof o === 'object') return { capture: !!o.capture, once: !!o.once, passive: !!o.passive, signal: o.signal };
  return { capture: false };
}
methods(ETP, {
  addEventListener(type, cb, opts) {
    const t = this ?? G;
    if (cb == null) return;
    const f = flagsOf(opts);
    if (f.signal && f.signal.aborted) return;
    type = String(type);
    let m = LISTENERS.get(t);
    if (!m) LISTENERS.set(t, m = new Map());
    let list = m.get(type);
    if (!list) m.set(type, list = []);
    if (list.some((l) => !l.handler && l.cb === cb && l.capture === f.capture)) return;
    list.push({ cb, capture: f.capture, once: !!f.once, passive: !!f.passive, removed: false });
    if (f.signal) f.signal.addEventListener('abort', () => t.removeEventListener(type, cb, f));
  },
  removeEventListener(type, cb, opts) {
    const t = this ?? G;
    const m = LISTENERS.get(t);
    const list = m && m.get(String(type));
    if (!list) return;
    const cap = flagsOf(opts).capture;
    const i = list.findIndex((l) => !l.handler && l.cb === cb && l.capture === cap);
    if (i >= 0) { list[i].removed = true; list.splice(i, 1); }
  },
  dispatchEvent(ev) {
    if (!(ev instanceof Event)) throw new TypeError("Failed to execute 'dispatchEvent' on 'EventTarget': parameter 1 is not of type 'Event'.");
    const s = EV.get(ev);
    if (s.dispatching) throw new DOMException('The event is already being dispatched.', 'InvalidStateError');
    s.trusted = false;
    return dispatch(this ?? G, ev);
  },
});

/// Handler `on*` de um alvo: o da propriedade ou o da variante (atributo
/// `onclick="..."`, `function onmessage() {}` no global...).
function handlerOf(t, type) {
  const h = HANDLERS.get(t);
  const set = h && h.get(type);
  if (typeof set === 'function') return set;
  if (set === null) return null;
  return inlineHandlerOf(t, type);
}
/// Chama o handler `on*` do alvo (se houver).
function callHandler(t, ev, s) {
  const h = handlerOf(t, s.type);
  if (!h) return;
  try {
    if (t === G && s.type === 'error' && ev instanceof ErrorEvent) {
      // onerror do global: (mensagem, arquivo, linha, coluna, erro); true cancela
      const r = h.call(t, ev.message, ev.filename, ev.lineno, ev.colno, ev.error);
      if (r === true) ev.preventDefault();
    } else {
      const r = h.call(t, ev);
      if (r === false && s.type !== 'mouseover' && s.type !== 'error') ev.preventDefault();
    }
  } catch (e) { N.reportError(e); }
}
function invoke(t, ev, s, phase) {
  s.currentTarget = t;
  const m = LISTENERS.get(t);
  const list = m && m.get(s.type);
  // o handler posto por propriedade tem lugar na lista (o do primeiro
  // `onX = f`); o do atributo HTML roda depois dos listeners
  let handled = false;
  if (list && list.length) {
    for (const l of list.slice()) {
      if (l.removed) continue;
      if (l.handler) {
        if (phase !== 1) { handled = true; callHandler(t, ev, s); }
        if (s.stopNow) return;
        continue;
      }
      if (phase === 1 && !l.capture) continue;
      if (phase === 3 && l.capture) continue;
      if (l.once) t.removeEventListener(s.type, l.cb, { capture: l.capture });
      s.passive = l.passive;
      try {
        if (typeof l.cb === 'function') l.cb.call(t, ev);
        else if (l.cb && typeof l.cb.handleEvent === 'function') l.cb.handleEvent(ev);
      } catch (e) { N.reportError(e); }
      s.passive = false;
      if (s.stopNow) return;
    }
  }
  if (phase !== 1 && !handled) callHandler(t, ev, s);
}
function dispatch(t, ev) {
  const s = EV.get(ev);
  s.dispatching = true; s.target = t; s.stop = s.stopNow = false;
  const path = eventPath(t, s);
  s.path = path;
  // window.event (só na janela)
  const prevEvent = IS_WINDOW ? G.event : undefined;
  if (IS_WINDOW) G.event = ev;
  try {
    s.phase = 1;
    for (let i = path.length - 1; i > 0 && !s.stop; i--) invoke(path[i], ev, s, 1);
    if (!s.stop) { s.phase = 2; invoke(path[0], ev, s, 2); }
    if (s.bubbles) {
      s.phase = 3;
      for (let i = 1; i < path.length && !s.stop; i++) invoke(path[i], ev, s, 3);
    }
  } finally {
    s.dispatching = false; s.phase = 0; s.currentTarget = null;
    if (IS_WINDOW) G.event = prevEvent;
  }
  return !s.canceled;
}
function fireTrusted(t, ev) {
  EV.get(ev).trusted = true;
  return dispatch(t, ev);
}

function defineHandlers(P, names) {
  for (const type of names) {
    accessor(P, 'on' + type, function () {
      const t = this ?? G;
      const h = HANDLERS.get(t);
      return (h && h.get(type)) || null;
    }, function (v) {
      const t = this ?? G;
      let h = HANDLERS.get(t);
      if (!h) HANDLERS.set(t, h = new Map());
      const fn = typeof v === 'function' ? v : null;
      const had = typeof h.get(type) === 'function';
      h.set(type, fn);
      // o lugar do handler entre os listeners é o do primeiro `onX = f`
      if (fn && !had) {
        let m = LISTENERS.get(t);
        if (!m) LISTENERS.set(t, m = new Map());
        let list = m.get(type);
        if (!list) m.set(type, list = []);
        list.push({ handler: true, capture: false, removed: false });
      } else if (!fn && had) {
        const list = LISTENERS.get(t)?.get(type);
        const i = list ? list.findIndex((l) => l.handler) : -1;
        if (i >= 0) { list[i].removed = true; list.splice(i, 1); }
      }
    });
  }
}

// ---------------------------------------------------- performance
const T0 = Date.now() - N.now();
const ENTRIES = [];
const PERF = Object.assign(new EventTarget(), {
  timeOrigin: T0,
  now() { return N.now(); },
  mark(name, opts) { const e = { name, entryType: 'mark', startTime: N.now(), duration: 0, detail: opts?.detail ?? null }; ENTRIES.push(e); return e; },
  measure(name, a, b) {
    const t = (x) => (typeof x === 'string' ? (ENTRIES.find((e) => e.name === x)?.startTime ?? 0) : 0);
    const start = t(a), end = b ? t(b) : N.now();
    const e = { name, entryType: 'measure', startTime: start, duration: end - start, detail: null };
    ENTRIES.push(e);
    return e;
  },
  getEntries() { return ENTRIES.slice(); },
  getEntriesByType(t) { return ENTRIES.filter((e) => e.entryType === t); },
  getEntriesByName(n, t) { return ENTRIES.filter((e) => e.name === n && (!t || e.entryType === t)); },
  clearMarks(n) { for (let i = ENTRIES.length - 1; i >= 0; i--) if (ENTRIES[i].entryType === 'mark' && (!n || ENTRIES[i].name === n)) ENTRIES.splice(i, 1); },
  clearMeasures(n) { for (let i = ENTRIES.length - 1; i >= 0; i--) if (ENTRIES[i].entryType === 'measure' && (!n || ENTRIES[i].name === n)) ENTRIES.splice(i, 1); },
  clearResourceTimings() {},
  setResourceTimingBufferSize() {},
  toJSON() { return { timeOrigin: T0 }; },
});
hidden(G, 'performance', PERF);
class PerformanceObserver {
  constructor(cb) { this.cb = cb; }
  observe() {} disconnect() {} takeRecords() { return []; }
  static get supportedEntryTypes() { return ['mark', 'measure']; }
}
hidden(G, 'PerformanceObserver', PerformanceObserver);

// ------------------------------------------------- structuredClone
// O clone em si é do Rust (script/clone.rs), também para postMessage. Aqui
// ficam os objetos da plataforma feitos em JS: o que guardar deles
// ([etiqueta, dados]; '!' = não clonável) e como remontá-los do outro lado.
const NOT_CLONEABLE = () => [EventTarget, Event, URL, URLSearchParams, Headers, Request, Response, FormData,
  AbortController, MessageChannel, TextEncoder, TextDecoder];
function cloneOut(o) {
  const b = BLOB.get(o);
  if (b) return o instanceof File ? ['File', [b.bytes, b.type, o.name, o.lastModified]] : ['Blob', [b.bytes, b.type]];
  const k = KEYS.get(o);
  if (k) return ['CryptoKey', [k.tipo, k.ext, k.alg, k.usos, k.m]];
  if (G.ImageData && o instanceof G.ImageData) return ['ImageData', [o.data, o.width, o.height]];
  // o global (window/self) é tratado como objeto JS: "#<Window>"
  if (o === G) return ['!', '#<' + Object.prototype.toString.call(o).slice(8, -1) + '>'];
  for (const C of NOT_CLONEABLE()) {
    if (o instanceof C) return ['!', (Object.getPrototypeOf(o)?.constructor?.name || C.name) + ' object'];
  }
  return undefined;
}
function cloneIn(tag, d) {
  switch (tag) {
    case 'Blob': return new Blob([d[0]], { type: d[1] });
    case 'File': return new File([d[0]], d[2], { type: d[1], lastModified: d[3] });
    case 'CryptoKey': return novaChave(d[0], d[1], d[2], d[3], d[4]);
    case 'ImageData': return G.ImageData ? new G.ImageData(d[0], d[1], d[2]) : { data: d[0], width: d[1], height: d[2] };
  }
  return undefined;
}
/// Lista de transferência de postMessage(msg, transfer) ou postMessage(msg, { transfer }).
function transferOf(t, ctx) {
  if (t === undefined || t === null) return [];
  if (typeof t !== 'object' && typeof t !== 'function') throw new TypeError(`${ctx}The provided value cannot be converted to a sequence.`);
  if (typeof t[Symbol.iterator] !== 'function') return t.transfer === undefined || t.transfer === null ? [] : [...t.transfer];
  return [...t];
}
hidden(G, 'structuredClone', function structuredClone(value, options) {
  const ctx = `Failed to execute 'structuredClone' on '${IS_WINDOW ? 'Window' : 'WorkerGlobalScope'}': `;
  if (arguments.length === 0) throw new TypeError(`${ctx}1 argument required, but only 0 present.`);
  const list = options === undefined || options === null ? [] : transferOf(options.transfer === undefined ? [] : options.transfer, ctx);
  return N.structuredClone(value, list, ctx, cloneOut, cloneIn);
});

// ------------------------------------------------- crypto
// Crypto: getRandomValues e randomUUID com o gerador do sistema operacional;
// o `subtle` (Web Crypto) vem na seção seguinte.
const TA_TAG = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag).get;
const INTEIROS = ['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array',
  'Uint32Array', 'BigInt64Array', 'BigUint64Array'];
function uuid() {
  const b = N.cryptoRandom(16);
  b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, '0')).join('');
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
class Crypto {
  constructor() { throw new TypeError('Illegal constructor'); }
  getRandomValues(arr) {
    const ctx = "Failed to execute 'getRandomValues' on 'Crypto': ";
    if (!arguments.length) throw new TypeError(`${ctx}1 argument required, but only 0 present.`);
    if (!ArrayBuffer.isView(arr)) throw new TypeError(`${ctx}parameter 1 is not of type 'ArrayBufferView'.`);
    const tipo = TA_TAG.call(arr) || 'DataView';
    if (!INTEIROS.includes(tipo)) {
      throw new DOMException(`${ctx}The provided ArrayBufferView is of type '${tipo}', which is not an integer array type.`, 'TypeMismatchError');
    }
    if (arr.byteLength > 65536) {
      throw new DOMException(`${ctx}The ArrayBufferView's byte length (${arr.byteLength}) exceeds the number of bytes of entropy available via this API (65536).`, 'QuotaExceededError');
    }
    new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength).set(N.cryptoRandom(arr.byteLength));
    return arr;
  }
  randomUUID() { return uuid(); }
  get subtle() { return SUBTLE; }
}
for (const n of ['getRandomValues', 'randomUUID', 'subtle']) Object.defineProperty(Crypto.prototype, n, { enumerable: true });
Object.defineProperty(Crypto.prototype, Symbol.toStringTag, { value: 'Crypto', configurable: true });
hidden(G, 'Crypto', Crypto);
hidden(G, 'crypto', Object.create(Crypto.prototype));

// ------------------------------------------------- Web Crypto (crypto.subtle)
// A matemática fica em Rust (script/crypto.rs, funções puras sobre bytes);
// aqui ficam os CryptoKey (o material vive no WeakMap KEYS, fora do alcance
// da página) e os métodos do SubtleCrypto; a normalização dos algoritmos, o
// JWK e os erros ficam em subtle.js, compilado no primeiro uso. A ordem das
// checagens e as mensagens seguem o Chrome (Blink + components/webcrypto).
const KEYS = new WeakMap();
let ATUAL = '';
const erroDe = (nome, msg) => (nome === 'TypeError' ? new TypeError(msg) : new DOMException(msg, nome));
// os erros que o Blink lança pelo ExceptionState levam o prefixo; os da operação, não
const ctxErro = (nome, msg) => erroDe(nome, `Failed to execute '${ATUAL}' on 'SubtleCrypto': ${msg}`);
// ---- CryptoKey
class CryptoKey {
  constructor() { throw new TypeError('Illegal constructor'); }
  get type() { return chave(this).tipo; }
  get extractable() { return chave(this).ext; }
  // [SameObject]: o mesmo objeto a cada leitura
  get algorithm() { const s = chave(this); return s.algJs || (s.algJs = algoritmoJs(s.alg)); }
  get usages() { const s = chave(this); return s.usosJs || (s.usosJs = s.usos.slice()); }
}
function chave(k) {
  const s = KEYS.get(k);
  if (!s) throw new TypeError('Illegal invocation');
  return s;
}
function novaChave(tipo, ext, alg, usos, m) {
  const k = Object.create(CryptoKey.prototype);
  KEYS.set(k, { tipo, ext, alg, usos, m });
  return k;
}
// a ordem das propriedades é a do Chrome: name, hash, length/modulusLength...
function algoritmoJs(a) {
  const o = { name: a.name };
  if (a.hash) o.hash = { name: a.hash };
  for (const k of ['length', 'modulusLength', 'namedCurve']) if (a[k] !== undefined) o[k] = a[k];
  if (a.publicExponent) o.publicExponent = a.publicExponent.slice();
  return o;
}
class SubtleCrypto {
  constructor() { throw new TypeError('Illegal constructor'); }
}
const SUBTLE = Object.create(SubtleCrypto.prototype);
// o corpo (normalização, JWK, algoritmos: subtle.js) só é compilado no
// primeiro uso; quase nenhuma página usa crypto.subtle
let METODOS = null;
const metodos = () => METODOS || (METODOS = N.subtle()(N, DOMException, KEYS, novaChave, erroDe, ctxErro, TA_TAG));
// nome → número mínimo de argumentos (o comprimento do método)
const SUBTLE_ARGS = { encrypt: 3, decrypt: 3, sign: 3, verify: 4, digest: 2, generateKey: 3, deriveKey: 5, deriveBits: 2,
  importKey: 5, exportKey: 2, wrapKey: 4, unwrapKey: 7 };
for (const [nome, minimo] of Object.entries(SUBTLE_ARGS)) {
  const f = { [nome](...args) {
    const antes = ATUAL;
    ATUAL = nome;
    try {
      if (this !== SUBTLE) throw new TypeError('Illegal invocation');
      if (args.length < minimo)
        throw ctxErro('TypeError', `${minimo} argument${minimo > 1 ? 's' : ''} required, but only ${args.length} present.`);
      return Promise.resolve(metodos()[nome][1](...args));
    } catch (e) {
      return Promise.reject(e);
    } finally {
      ATUAL = antes;
    }
  } }[nome];
  Object.defineProperty(f, 'length', { value: minimo });
  Object.defineProperty(SubtleCrypto.prototype, nome, { value: f, writable: true, enumerable: true, configurable: true });
}
// atributos e operações do WebIDL são enumeráveis no protótipo
for (const n of ['type', 'extractable', 'algorithm', 'usages']) Object.defineProperty(CryptoKey.prototype, n, { enumerable: true });
for (const [C, nome] of [[SubtleCrypto, 'SubtleCrypto'], [CryptoKey, 'CryptoKey']]) {
  Object.defineProperty(C.prototype, Symbol.toStringTag, { value: nome, configurable: true });
  hidden(G, nome, C);
}

// ------------------------------------------------- texto
class TextEncoder {
  get encoding() { return 'utf-8'; }
  encode(s = '') { return N.utf8Encode(String(s)); }
  encodeInto(s, dest) {
    const b = N.utf8Encode(String(s));
    const n = Math.min(b.length, dest.length);
    dest.set(b.subarray(0, n));
    return { read: String(s).length, written: n };
  }
}
class TextDecoder {
  constructor(label = 'utf-8', opts = {}) {
    this.encoding = String(label).toLowerCase().replace(/^utf8$/, 'utf-8');
    this.fatal = !!opts.fatal;
    this.ignoreBOM = !!opts.ignoreBOM;
  }
  decode(buf) { return buf === undefined ? '' : N.utf8Decode(buf, this.encoding); }
}
hidden(G, 'TextEncoder', TextEncoder);
hidden(G, 'TextDecoder', TextDecoder);

// ------------------------------------------------- URL e URLSearchParams
const URLS = new WeakMap();
class URLSearchParams {
  constructor(init) {
    const list = [];
    URLS.set(this, { list, owner: null });
    if (init === undefined || init === null) return;
    if (typeof init === 'object' && typeof init[Symbol.iterator] === 'function') {
      for (const pair of init) {
        const p = [...pair];
        if (p.length !== 2) throw new TypeError("Failed to construct 'URLSearchParams': Sequence initializer must only contain pair elements");
        list.push([String(p[0]), String(p[1])]);
      }
    } else if (typeof init === 'object') {
      for (const k of Object.keys(init)) list.push([k, String(init[k])]);
    } else {
      parseQuery(String(init), list);
    }
  }
  append(k, v) { URLS.get(this).list.push([String(k), String(v)]); syncURL(this); }
  delete(k, v) {
    const s = URLS.get(this);
    s.list = s.list.filter(([a, b]) => !(a === String(k) && (v === undefined || b === String(v))));
    syncURL(this);
  }
  get(k) { const e = URLS.get(this).list.find(([a]) => a === String(k)); return e ? e[1] : null; }
  getAll(k) { return URLS.get(this).list.filter(([a]) => a === String(k)).map((e) => e[1]); }
  has(k, v) { return URLS.get(this).list.some(([a, b]) => a === String(k) && (v === undefined || b === String(v))); }
  set(k, v) {
    k = String(k); v = String(v);
    const s = URLS.get(this);
    const i = s.list.findIndex(([a]) => a === k);
    if (i < 0) s.list.push([k, v]);
    else { s.list[i][1] = v; s.list = s.list.filter(([a], j) => a !== k || j === i); }
    syncURL(this);
  }
  sort() { URLS.get(this).list.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)); syncURL(this); }
  forEach(cb, thisArg) { for (const [k, v] of URLS.get(this).list) cb.call(thisArg, v, k, this); }
  keys() { return URLS.get(this).list.map((e) => e[0]).values(); }
  values() { return URLS.get(this).list.map((e) => e[1]).values(); }
  entries() { return URLS.get(this).list.map((e) => [e[0], e[1]]).values(); }
  [Symbol.iterator]() { return this.entries(); }
  get size() { return URLS.get(this).list.length; }
  toString() { return URLS.get(this).list.map(([k, v]) => N.formEncode(k) + '=' + N.formEncode(v)).join('&'); }
}
function parseQuery(q, list) {
  if (q.startsWith('?')) q = q.slice(1);
  for (const part of q.split('&')) {
    if (!part) continue;
    const i = part.indexOf('=');
    const k = i < 0 ? part : part.slice(0, i);
    const v = i < 0 ? '' : part.slice(i + 1);
    list.push([N.formDecode(k), N.formDecode(v)]);
  }
}
function syncURL(params) {
  const s = URLS.get(params);
  if (!s.owner) return;
  const q = params.toString();
  URL_DATA.get(s.owner).parts = N.urlParse(N.urlSet(s.owner.href, 'search', q));
}
const URL_DATA = new WeakMap();
class URL {
  constructor(url, base) {
    if (arguments.length === 0) throw new TypeError("Failed to construct 'URL': 1 argument required, but only 0 present.");
    const b = base === undefined ? undefined : String(base);
    const parts = N.urlParse(String(url), b);
    if (!parts) throw new TypeError(`Failed to construct 'URL': Invalid URL`);
    URL_DATA.set(this, { parts, params: null });
  }
  static canParse(u, b) { return !!N.urlParse(String(u), b === undefined ? undefined : String(b)); }
  static parse(u, b) { try { return new URL(u, b); } catch (e) { return null; } }
  static createObjectURL(obj) {
    const id = uuid();
    const url = `blob:${G.origin}/${id}`;
    // Blob: no registro do Rust (vale para fetch, workers, <img>, <video>...)
    const b = BLOB.get(obj);
    if (b) N.blobRegister(url, b.bytes, b.type);
    else if (G.MediaSource && obj instanceof G.MediaSource) BLOBS.set(id, obj);
    else throw new TypeError("Failed to execute 'createObjectURL' on 'URL': Overload resolution failed.");
    return url;
  }
  static revokeObjectURL(u) {
    u = String(u);
    N.blobRevoke(u);
    BLOBS.delete(u.split('/').pop());
  }
  get searchParams() {
    const d = URL_DATA.get(this);
    if (!d.params) {
      d.params = new URLSearchParams(d.parts.search);
      URLS.get(d.params).owner = this;
    }
    return d.params;
  }
  toString() { return this.href; }
  toJSON() { return this.href; }
}
// blob: de MediaSource (o objeto não sai da realm)
const BLOBS = new Map();
for (const part of ['href', 'protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin']) {
  accessor(URL.prototype, part, function () { return URL_DATA.get(this).parts[part]; }, part === 'origin' ? undefined : function (v) {
    const d = URL_DATA.get(this);
    const href = part === 'href' ? String(v) : N.urlSet(d.parts.href, part, String(v));
    const p = N.urlParse(href);
    if (!p) { if (part === 'href') throw new TypeError(`Failed to set the 'href' property on 'URL': Invalid URL`); return; }
    d.parts = p;
    if (d.params && (part === 'search' || part === 'href')) {
      URLS.get(d.params).list = [];
      parseQuery(p.search, URLS.get(d.params).list);
    }
  });
}
hidden(G, 'URL', URL);
hidden(G, 'webkitURL', URL);
hidden(G, 'URLSearchParams', URLSearchParams);

// ------------------------------------------------- Blob, File, FormData
const BLOB = new WeakMap();
function toBytes(part) {
  if (part instanceof Blob) return BLOB.get(part).bytes;
  if (part instanceof ArrayBuffer) return new Uint8Array(part.slice(0));
  if (ArrayBuffer.isView(part)) return new Uint8Array(part.buffer.slice(part.byteOffset, part.byteOffset + part.byteLength));
  return N.utf8Encode(String(part));
}
class Blob {
  constructor(parts = [], opts = {}) {
    const chunks = [...parts].map(toBytes);
    const size = chunks.reduce((n, c) => n + c.length, 0);
    const bytes = new Uint8Array(size);
    let o = 0;
    for (const c of chunks) { bytes.set(c, o); o += c.length; }
    BLOB.set(this, { bytes, type: String(opts.type || '').toLowerCase() });
  }
  get size() { return BLOB.get(this).bytes.length; }
  get type() { return BLOB.get(this).type; }
  text() { return Promise.resolve(N.utf8Decode(BLOB.get(this).bytes)); }
  arrayBuffer() { const b = BLOB.get(this).bytes; return Promise.resolve(b.buffer.slice(b.byteOffset, b.byteOffset + b.length)); }
  bytes() { return Promise.resolve(BLOB.get(this).bytes.slice()); }
  slice(start = 0, end, type = '') {
    const b = BLOB.get(this).bytes;
    return new Blob([b.slice(start, end === undefined ? b.length : end)], { type });
  }
  stream() { throw new TypeError('Blob.stream() ainda não é suportado.'); }
}
class File extends Blob {
  constructor(parts, name, opts = {}) {
    super(parts, opts);
    this.name = String(name);
    this.lastModified = opts.lastModified ?? Date.now();
  }
}
hidden(G, 'Blob', Blob);
hidden(G, 'File', File);
class FormData {
  constructor(form, submitter) {
    this._list = [];
    if (form) for (const [k, v] of formEntriesOf(form, submitter)) this._list.push([k, v]);
  }
  append(k, v, name) { this._list.push([String(k), v instanceof Blob ? v : String(v), name]); }
  delete(k) { this._list = this._list.filter(([a]) => a !== String(k)); }
  get(k) { const e = this._list.find(([a]) => a === String(k)); return e ? e[1] : null; }
  getAll(k) { return this._list.filter(([a]) => a === String(k)).map((e) => e[1]); }
  has(k) { return this._list.some(([a]) => a === String(k)); }
  set(k, v, name) {
    k = String(k);
    const i = this._list.findIndex(([a]) => a === k);
    const entry = [k, v instanceof Blob ? v : String(v), name];
    if (i < 0) this._list.push(entry);
    else { this._list[i] = entry; this._list = this._list.filter(([a], j) => a !== k || j === i); }
  }
  forEach(cb, t) { for (const [k, v] of this._list) cb.call(t, v, k, this); }
  keys() { return this._list.map((e) => e[0]).values(); }
  values() { return this._list.map((e) => e[1]).values(); }
  entries() { return this._list.map((e) => [e[0], e[1]]).values(); }
  [Symbol.iterator]() { return this.entries(); }
}
hidden(G, 'FormData', FormData);

// ------------------------------------------------- FileReader
const READER = new WeakMap();
function binaryOf(b) {
  let s = '';
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode.apply(null, b.subarray(i, i + 0x8000));
  return s;
}
function readBlob(fr, blob, how, enc) {
  const s = READER.get(fr);
  if (!(blob instanceof Blob)) throw new TypeError(`Failed to execute '${how}' on 'FileReader': parameter 1 is not of type 'Blob'.`);
  if (s.state === 1) throw new DOMException(`Failed to execute '${how}' on 'FileReader': The object is already busy reading Blobs.`, 'InvalidStateError');
  s.state = 1; s.result = null; s.error = null;
  const seq = ++s.seq;
  const bytes = BLOB.get(blob).bytes, type = blob.type, n = bytes.length;
  const fire = (t) => fireTrusted(fr, new ProgressEvent(t, { lengthComputable: true, loaded: n, total: n }));
  setTimeout(() => {
    if (s.seq !== seq) return;
    fire('loadstart');
    if (how === 'readAsArrayBuffer') s.result = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + n);
    else if (how === 'readAsText') s.result = N.utf8Decode(bytes, enc === undefined ? 'utf-8' : String(enc));
    else if (how === 'readAsDataURL') s.result = `data:${type || 'application/octet-stream'};base64,${btoa(binaryOf(bytes))}`;
    else s.result = binaryOf(bytes);
    s.state = 2;
    fire('progress');
    fire('load');
    fire('loadend');
  }, 0);
}
class FileReader extends EventTarget {
  constructor() { super(); READER.set(this, { state: 0, result: null, error: null, seq: 0 }); }
  get readyState() { return READER.get(this).state; }
  get result() { return READER.get(this).result; }
  get error() { return READER.get(this).error; }
  readAsArrayBuffer(b) { readBlob(this, b, 'readAsArrayBuffer'); }
  readAsText(b, enc) { readBlob(this, b, 'readAsText', enc); }
  readAsDataURL(b) { readBlob(this, b, 'readAsDataURL'); }
  readAsBinaryString(b) { readBlob(this, b, 'readAsBinaryString'); }
  abort() {
    const s = READER.get(this);
    if (s.state !== 1) return;
    s.seq++; s.state = 2; s.result = null;
    fireTrusted(this, new ProgressEvent('abort'));
    fireTrusted(this, new ProgressEvent('loadend'));
  }
}
for (const [k, v] of [['EMPTY', 0], ['LOADING', 1], ['DONE', 2]]) {
  Object.defineProperty(FileReader, k, { value: v });
  Object.defineProperty(FileReader.prototype, k, { value: v });
}
defineHandlers(FileReader.prototype, ['loadstart', 'progress', 'load', 'abort', 'error', 'loadend']);
hidden(G, 'FileReader', FileReader);

// ------------------------------------------------- Abort
class AbortSignal extends EventTarget {
  constructor() { super(); this.aborted = false; this.reason = undefined; this.onabort = null; }
  throwIfAborted() { if (this.aborted) throw this.reason; }
  static abort(reason) { const c = new AbortController(); c.abort(reason); return c.signal; }
  static timeout(ms) {
    const c = new AbortController();
    setTimeout(() => c.abort(new DOMException('signal timed out', 'TimeoutError')), ms);
    return c.signal;
  }
  static any(signals) {
    const c = new AbortController();
    for (const s of signals) {
      if (s.aborted) { c.abort(s.reason); break; }
      s.addEventListener('abort', () => c.abort(s.reason));
    }
    return c.signal;
  }
}
class AbortController {
  constructor() { this.signal = new AbortSignal(); }
  abort(reason) {
    const s = this.signal;
    if (s.aborted) return;
    s.aborted = true;
    s.reason = reason === undefined ? new DOMException('signal is aborted without reason', 'AbortError') : reason;
    const ev = new Event('abort');
    if (typeof s.onabort === 'function') s.onabort(ev);
    s.dispatchEvent(ev);
  }
}
hidden(G, 'AbortSignal', AbortSignal);
hidden(G, 'AbortController', AbortController);

// ------------------------------------------------- fetch
class Headers {
  constructor(init) {
    this._h = [];
    if (init instanceof Headers) this._h = init._h.map((e) => e.slice());
    else if (init && typeof init[Symbol.iterator] === 'function') for (const [k, v] of init) this.append(k, v);
    else if (init && typeof init === 'object') for (const k of Object.keys(init)) this.append(k, init[k]);
  }
  append(k, v) { this._h.push([String(k).toLowerCase(), String(v).trim()]); }
  set(k, v) { k = String(k).toLowerCase(); this.delete(k); this._h.push([k, String(v).trim()]); }
  get(k) {
    k = String(k).toLowerCase();
    const vs = this._h.filter((e) => e[0] === k).map((e) => e[1]);
    return vs.length ? vs.join(', ') : null;
  }
  getSetCookie() { return []; }
  has(k) { k = String(k).toLowerCase(); return this._h.some((e) => e[0] === k); }
  delete(k) { k = String(k).toLowerCase(); this._h = this._h.filter((e) => e[0] !== k); }
  forEach(cb, t) { for (const [k, v] of this.entries()) cb.call(t, v, k, this); }
  *entries() {
    const names = [...new Set(this._h.map((e) => e[0]))].sort();
    for (const n of names) yield [n, this.get(n)];
  }
  keys() { return [...this.entries()].map((e) => e[0]).values(); }
  values() { return [...this.entries()].map((e) => e[1]).values(); }
  [Symbol.iterator]() { return this.entries(); }
}
hidden(G, 'Headers', Headers);

/// Corpo de uma requisição: bytes (ou null) e o content-type implícito.
function encodeBody(body) {
  if (body === undefined || body === null) return [null, null];
  if (typeof body === 'string') return [body, 'text/plain;charset=UTF-8'];
  if (body instanceof URLSearchParams) return [body.toString(), 'application/x-www-form-urlencoded;charset=UTF-8'];
  if (body instanceof Blob) return [BLOB.get(body).bytes, body.type || null];
  if (body instanceof ArrayBuffer || ArrayBuffer.isView(body)) return [toBytes(body), null];
  if (body instanceof FormData) {
    const boundary = '----FerroFormBoundary' + uuid().replace(/-/g, '');
    const parts = [];
    for (const [k, v, name] of body._list) {
      let head = `--${boundary}\r\nContent-Disposition: form-data; name="${k}"`;
      if (v instanceof Blob) {
        head += `; filename="${name || v.name || 'blob'}"\r\nContent-Type: ${v.type || 'application/octet-stream'}`;
        parts.push(head + '\r\n\r\n', v, '\r\n');
      } else parts.push(head + '\r\n\r\n' + v + '\r\n');
    }
    parts.push(`--${boundary}--\r\n`);
    return [BLOB.get(new Blob(parts)).bytes, 'multipart/form-data; boundary=' + boundary];
  }
  return [String(body), 'text/plain;charset=UTF-8'];
}
class Request {
  constructor(input, init = {}) {
    const src = input instanceof Request ? input : null;
    const url = src ? src.url : String(input);
    const p = N.urlParse(url, baseURL());
    if (!p) throw new TypeError(`Failed to construct 'Request': Invalid URL`);
    this.url = p.href;
    this.method = String(init.method || (src && src.method) || 'GET').toUpperCase();
    this.headers = new Headers(init.headers || (src && src.headers) || undefined);
    this._body = init.body !== undefined ? init.body : src ? src._body : null;
    this.credentials = init.credentials || (src && src.credentials) || 'same-origin';
    this.mode = init.mode || (src && src.mode) || 'cors';
    this.cache = init.cache || 'default';
    this.redirect = init.redirect || 'follow';
    this.referrer = 'about:client';
    this.referrerPolicy = init.referrerPolicy || '';
    this.integrity = init.integrity || '';
    this.keepalive = !!init.keepalive;
    this.signal = init.signal || (src && src.signal) || new AbortSignal();
    this.destination = '';
  }
  clone() { return new Request(this); }
  text() { return Promise.resolve(this._body == null ? '' : typeof this._body === 'string' ? this._body : N.utf8Decode(encodeBody(this._body)[0])); }
  json() { return this.text().then(JSON.parse); }
  arrayBuffer() { return Promise.resolve(toBytes(encodeBody(this._body)[0] ?? '').buffer); }
}
hidden(G, 'Request', Request);
const STATUS_TEXT = { 200: 'OK', 201: 'Created', 204: 'No Content', 206: 'Partial Content', 301: 'Moved Permanently',
  302: 'Found', 304: 'Not Modified', 400: 'Bad Request', 401: 'Unauthorized', 403: 'Forbidden', 404: 'Not Found',
  405: 'Method Not Allowed', 429: 'Too Many Requests', 500: 'Internal Server Error', 502: 'Bad Gateway', 503: 'Service Unavailable' };
class Response {
  constructor(body = null, init = {}) {
    const [bytes, ct] = encodeBody(body);
    this._bytes = bytes === null ? new Uint8Array(0) : typeof bytes === 'string' ? N.utf8Encode(bytes) : bytes;
    this.status = init.status ?? 200;
    this.statusText = init.statusText ?? STATUS_TEXT[this.status] ?? '';
    this.headers = new Headers(init.headers);
    if (ct && !this.headers.has('content-type')) this.headers.set('content-type', ct);
    this.url = '';
    this.redirected = false;
    this.type = 'default';
    this.bodyUsed = false;
  }
  get ok() { return this.status >= 200 && this.status < 300; }
  get body() {
    const bytes = this._bytes;
    let done = false;
    return {
      locked: false,
      getReader() {
        return {
          read() {
            if (done) return Promise.resolve({ done: true, value: undefined });
            done = true;
            return Promise.resolve({ done: false, value: bytes });
          },
          releaseLock() {}, cancel() { done = true; return Promise.resolve(); }, closed: Promise.resolve(),
        };
      },
      cancel() { return Promise.resolve(); },
    };
  }
  _use() {
    if (this.bodyUsed) return Promise.reject(new TypeError('Failed to execute on \'Response\': body stream already read'));
    this.bodyUsed = true;
    return Promise.resolve(this._bytes);
  }
  text() { return this._use().then((b) => N.utf8Decode(b, charsetOf(this.headers.get('content-type')))); }
  json() { return this.text().then((t) => JSON.parse(t)); }
  arrayBuffer() { return this._use().then((b) => b.buffer.slice(b.byteOffset, b.byteOffset + b.length)); }
  bytes() { return this._use().then((b) => b.slice()); }
  blob() { return this._use().then((b) => new Blob([b], { type: this.headers.get('content-type') || '' })); }
  formData() { return this.text().then((t) => { const f = new FormData(); for (const [k, v] of new URLSearchParams(t)) f.append(k, v); return f; }); }
  clone() {
    const r = new Response(this._bytes.slice(), { status: this.status, statusText: this.statusText, headers: this.headers });
    r.url = this.url; r.redirected = this.redirected; r.type = this.type;
    return r;
  }
  static json(data, init = {}) {
    const h = new Headers(init.headers);
    h.set('content-type', 'application/json');
    return new Response(JSON.stringify(data), { ...init, headers: h });
  }
  static error() { const r = new Response(null, { status: 0 }); r.type = 'error'; return r; }
  static redirect(url, status = 302) { return new Response(null, { status, headers: { location: String(url) } }); }
}
function charsetOf(ct) {
  const m = /charset=["']?([\w-]+)/i.exec(ct || '');
  return m ? m[1] : 'utf-8';
}
hidden(G, 'Response', Response);
function fromNative(r, url) {
  const res = new Response(null, { status: r.status, headers: r.headers });
  res._bytes = r.body || new Uint8Array(0);
  res.type = r.type || 'basic';
  // resposta opaca (no-cors de outro site): nada dela fica visível
  res.url = res.type === 'opaque' ? '' : r.url || url;
  res.redirected = res.type !== 'opaque' && !!r.url && r.url !== url;
  if (res.type === 'opaque') res.statusText = '';
  return res;
}
// bloqueio de CORS: o motivo vai para o console, como no Chrome
function corsError(r) {
  if (r.error && r.error.startsWith('Access to ')) console.error(r.error);
}
hidden(G, 'fetch', function fetch(input, init) {
  return new Promise((resolve, reject) => {
    const req = new Request(input, init);
    if (req.signal.aborted) { reject(req.signal.reason); return; }
    if (req.url.startsWith('blob:')) {
      // blob URL: do registro do Rust (pode ter sido criado em outra realm)
      const b = req.method === 'GET' ? N.blobGet(req.url) : null;
      if (!b) { reject(new TypeError('Failed to fetch')); return; }
      const h = [['content-length', String(b[0].length)]];
      if (b[1]) h.push(['content-type', b[1]]);
      const res = new Response(null, { status: 200, headers: h });
      res._bytes = b[0];
      res.url = req.url;
      res.type = 'basic';
      resolve(res);
      return;
    }
    const [bytes, ct] = encodeBody(req._body);
    if (ct && !req.headers.has('content-type')) req.headers.set('content-type', ct);
    let finished = false;
    req.signal.addEventListener('abort', () => {
      if (!finished) { finished = true; reject(req.signal.reason); }
    });
    N.fetch(req.method, req.url, [...req.headers], bytes, req.credentials, (r) => {
      if (finished) return;
      finished = true;
      if (r.error) { corsError(r); reject(new TypeError('Failed to fetch')); }
      else resolve(fromNative(r, req.url));
    }, req.mode, 'fetch');
  });
});

// ------------------------------------------------- XMLHttpRequest
class XMLHttpRequestEventTarget extends EventTarget {}
class XMLHttpRequest extends XMLHttpRequestEventTarget {
  constructor() {
    super();
    this.readyState = 0;
    this.status = 0;
    this.statusText = '';
    this.responseType = '';
    this.responseURL = '';
    this.timeout = 0;
    this.withCredentials = false;
    this.upload = new XMLHttpRequestEventTarget();
    this._headers = [];
    this._res = null;
    this._seq = 0;
  }
  open(method, url, async = true) {
    const p = N.urlParse(String(url), baseURL());
    if (!p) throw new DOMException(`Failed to execute 'open' on 'XMLHttpRequest': Invalid URL`, 'SyntaxError');
    this._method = String(method).toUpperCase();
    this._url = p.href;
    this._async = async !== false;
    this._headers = [];
    this._res = null;
    this._seq++;
    this.status = 0;
    this._state(1);
  }
  setRequestHeader(k, v) {
    if (this.readyState !== 1) throw new DOMException("Failed to execute 'setRequestHeader' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError');
    this._headers.push([String(k), String(v)]);
  }
  _state(n) {
    this.readyState = n;
    this.dispatchEvent(new Event('readystatechange'));
  }
  _fire(type, extra) {
    this.dispatchEvent(new ProgressEvent(type, extra || {}));
  }
  send(body) {
    if (this.readyState !== 1) throw new DOMException("Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError');
    const [bytes, ct] = encodeBody(this._method === 'GET' || this._method === 'HEAD' ? null : body);
    const headers = this._headers.slice();
    if (ct && !headers.some(([k]) => k.toLowerCase() === 'content-type')) headers.push(['Content-Type', ct]);
    const done = (r) => {
      if (r.error) {
        corsError(r);
        this._state(4);
        this._fire('error');
        this._fire('loadend');
        return;
      }
      this._res = r;
      this.status = r.status;
      this.statusText = STATUS_TEXT[r.status] ?? '';
      this.responseURL = r.url;
      this._state(2);
      this._state(3);
      const total = r.body ? r.body.length : 0;
      this._fire('progress', { lengthComputable: true, loaded: total, total });
      this._state(4);
      this._fire('load', { lengthComputable: true, loaded: total, total });
      this._fire('loadend', { lengthComputable: true, loaded: total, total });
    };
    this._fire('loadstart');
    if (!this._async) {
      done(N.fetchSync(this._method, this._url, headers, bytes, this.withCredentials ? 'include' : 'same-origin', null, 'cors', 'XMLHttpRequest'));
      return;
    }
    const seq = this._seq;
    N.fetch(this._method, this._url, headers, bytes, this.withCredentials ? 'include' : 'same-origin', (r) => {
      if (seq === this._seq) done(r);
    }, 'cors', 'XMLHttpRequest');
    if (this.timeout > 0) {
      setTimeout(() => {
        if (seq === this._seq && this.readyState !== 4) {
          this._seq++;
          this._state(4);
          this._fire('timeout');
          this._fire('loadend');
        }
      }, this.timeout);
    }
  }
  abort() {
    if (this.readyState === 0 || this.readyState === 4) return;
    this._seq++;
    this._state(4);
    this.readyState = 0;
    this._fire('abort');
    this._fire('loadend');
  }
  overrideMimeType(t) { this._mime = t; }
  getResponseHeader(k) {
    if (!this._res) return null;
    k = String(k).toLowerCase();
    const vs = this._res.headers.filter(([a]) => a === k).map((e) => e[1]);
    return vs.length ? vs.join(', ') : null;
  }
  getAllResponseHeaders() {
    if (!this._res) return '';
    return this._res.headers.map(([k, v]) => `${k}: ${v}\r\n`).join('');
  }
  get responseText() {
    if (this.responseType !== '' && this.responseType !== 'text') throw new DOMException("Failed to read the 'responseText' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'text'.", 'InvalidStateError');
    if (!this._res || !this._res.body) return '';
    if (this._text === undefined || this._textFor !== this._res) {
      this._textFor = this._res;
      this._text = N.utf8Decode(this._res.body, charsetOf(this._mime || this._res.contentType));
    }
    return this._text;
  }
  get response() {
    if (!this._res || this.readyState !== 4) return this.responseType === '' || this.responseType === 'text' ? '' : null;
    const b = this._res.body || new Uint8Array(0);
    switch (this.responseType) {
      case 'json': try { return JSON.parse(this.responseText_()); } catch (e) { return null; }
      case 'arraybuffer': return b.buffer.slice(b.byteOffset, b.byteOffset + b.length);
      case 'blob': return new Blob([b], { type: this.getResponseHeader('content-type') || '' });
      case 'document': return null;
      default: return this.responseText;
    }
  }
  responseText_() { return N.utf8Decode(this._res.body, charsetOf(this._res.contentType)); }
  get responseXML() { return null; }
}
for (const [k, v] of [['UNSENT', 0], ['OPENED', 1], ['HEADERS_RECEIVED', 2], ['LOADING', 3], ['DONE', 4]]) {
  Object.defineProperty(XMLHttpRequest, k, { value: v });
  Object.defineProperty(XMLHttpRequest.prototype, k, { value: v });
}
// on* das requisições: propriedades comuns, achadas pelo despacho
for (const P of [XMLHttpRequestEventTarget.prototype]) {
  defineHandlers(P, ['loadstart', 'progress', 'abort', 'error', 'load', 'timeout', 'loadend', 'readystatechange']);
}
defineHandlers(AbortSignal.prototype, []);
hidden(G, 'XMLHttpRequestEventTarget', XMLHttpRequestEventTarget);
hidden(G, 'XMLHttpRequest', XMLHttpRequest);

// ------------------------------------------------- mensagens
// MessagePort, BroadcastChannel e Worker: as mensagens vão pelo Rust
// (script/messaging.rs), serializadas, e podem atravessar threads.
const PORT_STATE = new WeakMap();
function portState(p) {
  let s = PORT_STATE.get(p);
  if (!s) PORT_STATE.set(p, s = { started: false, queue: [] });
  return s;
}
/// Liga a fila da porta (start() ou onmessage): o que esperava sai em ordem.
function startPort(p) {
  const s = portState(p);
  if (s.started) return;
  s.started = true;
  for (const ev of s.queue.splice(0)) setTimeout(() => fireTrusted(p, ev), 0);
}
class MessagePort extends EventTarget {
  constructor() { throw new TypeError('Illegal constructor'); }
  postMessage(message, transfer) {
    const ctx = "Failed to execute 'postMessage' on 'MessagePort': ";
    if (arguments.length === 0) throw new TypeError(`${ctx}1 argument required, but only 0 present.`);
    N.portPost(this, message, transferOf(transfer, ctx), cloneOut);
  }
  start() { startPort(this); }
  close() { N.portClose(this); }
}
defineHandlers(MessagePort.prototype, ['message', 'messageerror']);
{
  const d = Object.getOwnPropertyDescriptor(MessagePort.prototype, 'onmessage');
  accessor(MessagePort.prototype, 'onmessage', d.get, function (v) { d.set.call(this, v); startPort(this); });
}
N.portSetup(MessagePort.prototype);
const CHANNEL = new WeakMap();
class MessageChannel {
  constructor() { CHANNEL.set(this, N.channelNew()); }
  get port1() { return CHANNEL.get(this)[0]; }
  get port2() { return CHANNEL.get(this)[1]; }
}
hidden(G, 'MessagePort', MessagePort);
hidden(G, 'MessageChannel', MessageChannel);
const BCAST = new WeakMap();
class BroadcastChannel extends EventTarget {
  constructor(name) {
    if (arguments.length === 0) throw new TypeError("Failed to construct 'BroadcastChannel': 1 argument required, but only 0 present.");
    super();
    const n = String(name);
    BCAST.set(this, { name: n, id: N.bcOpen(this, n), closed: false });
  }
  get name() { return BCAST.get(this).name; }
  postMessage(message) {
    const ctx = "Failed to execute 'postMessage' on 'BroadcastChannel': ";
    if (arguments.length === 0) throw new TypeError(`${ctx}1 argument required, but only 0 present.`);
    const s = BCAST.get(this);
    if (s.closed) throw new DOMException(`${ctx}Channel is closed`, 'InvalidStateError');
    N.bcPost(s.id, message, cloneOut);
  }
  close() {
    const s = BCAST.get(this);
    if (!s.closed) { s.closed = true; N.bcClose(s.id); }
  }
}
defineHandlers(BroadcastChannel.prototype, ['message', 'messageerror']);
hidden(G, 'BroadcastChannel', BroadcastChannel);
const WORKER_ID = new WeakMap();
class Worker extends EventTarget {
  constructor(url, options) {
    const ctx = "Failed to construct 'Worker': ";
    if (arguments.length === 0) throw new TypeError(`${ctx}1 argument required, but only 0 present.`);
    super();
    const o = options === undefined || options === null ? {} : options;
    const type = o.type === undefined ? 'classic' : String(o.type);
    if (type !== 'classic' && type !== 'module') {
      throw new TypeError(`${ctx}Failed to read the 'type' property from 'WorkerOptions': The provided value '${type}' is not a valid enum value of type WorkerType.`);
    }
    WORKER_ID.set(this, N.workerNew(this, String(url), type, o.name === undefined ? '' : String(o.name)));
  }
  postMessage(message, transfer) {
    const ctx = "Failed to execute 'postMessage' on 'Worker': ";
    if (arguments.length === 0) throw new TypeError(`${ctx}1 argument required, but only 0 present.`);
    N.workerPost(WORKER_ID.get(this), message, transferOf(transfer, ctx), cloneOut);
  }
  terminate() { N.workerTerminate(WORKER_ID.get(this)); }
}
defineHandlers(Worker.prototype, ['message', 'messageerror', 'error']);
hidden(G, 'Worker', Worker);

// ---------------------------------------------------- ganchos do Rust
// (cada variante acrescenta os seus e publica em __ferroHooks no fim)
const hooks = {
  cloneIn,
  portMessage(port, data, ports) {
    const ev = ports === null ? new MessageEvent('messageerror', { data: null })
      : new MessageEvent('message', { data, ports: Object.freeze(ports) });
    const s = portState(port);
    if (!s.started) { s.queue.push(ev); return; }
    fireTrusted(port, ev);
  },
  broadcastMessage(bc, data, origin) {
    const s = BCAST.get(bc);
    if (!s || s.closed) return;
    fireTrusted(bc, origin === null ? new MessageEvent('messageerror', { data: null }) : new MessageEvent('message', { data, origin }));
  },
  workerEvent(w, kind, a, b, c, d) {
    if (kind === 'message') fireTrusted(w, new MessageEvent('message', { data: a, ports: Object.freeze(b) }));
    else if (kind === 'messageerror') fireTrusted(w, new MessageEvent('messageerror', { data: null }));
    else if (kind === 'error') return fireTrusted(w, new ErrorEvent('error', { message: a, filename: b, lineno: c, colno: d, error: null, cancelable: true }));
    else if (kind === 'loaderror') fireTrusted(w, new Event('error', { cancelable: true }));
    return true;
  },
};
