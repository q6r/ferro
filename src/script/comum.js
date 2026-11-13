// Prelúdio comum: a parte da plataforma que não depende do DOM (eventos,
// DOMException, clone estruturado, crypto, texto, URL, Blob, fetch/XHR,
// mensagens). O Rust junta este arquivo com o `prelude.js` (página), dentro
// da mesma função `(function (N) { ... })(__native)`.
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
hidden(G, 'isSecureContext', true);
hidden(G, 'origin', N.urlParse(N.href())?.origin ?? 'null');
hidden(G, 'reportError', (e) => N.reportError(e));

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
eventClass('ErrorEvent', Event, { message: '', filename: '', lineno: 0, colno: 0, error: null });
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
    if (list.some((l) => l.cb === cb && l.capture === f.capture)) return;
    list.push({ cb, capture: f.capture, once: !!f.once, passive: !!f.passive, removed: false });
    if (f.signal) f.signal.addEventListener('abort', () => t.removeEventListener(type, cb, f));
  },
  removeEventListener(type, cb, opts) {
    const t = this ?? G;
    const m = LISTENERS.get(t);
    const list = m && m.get(String(type));
    if (!list) return;
    const cap = flagsOf(opts).capture;
    const i = list.findIndex((l) => l.cb === cb && l.capture === cap);
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
function invoke(t, ev, s, phase) {
  s.currentTarget = t;
  const m = LISTENERS.get(t);
  const list = m && m.get(s.type);
  if (list && list.length) {
    for (const l of list.slice()) {
      if (l.removed) continue;
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
  if (phase !== 1) {
    const h = handlerOf(t, s.type);
    if (h) {
      try {
        const r = h.call(t, ev);
        if (r === false && s.type !== 'mouseover' && s.type !== 'error') ev.preventDefault();
      } catch (e) { N.reportError(e); }
    }
  }
}
function dispatch(t, ev) {
  const s = EV.get(ev);
  s.dispatching = true; s.target = t; s.stop = s.stopNow = false;
  const path = eventPath(t, s);
  s.path = path;
  const prevEvent = G.event;
  G.event = ev;
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
    G.event = prevEvent;
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
      h.set(type, typeof v === 'function' ? v : null);
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
function clone(v, seen) {
  if (typeof v !== 'object' || v === null) {
    if (typeof v === 'function' || typeof v === 'symbol')
      throw new DOMException(`${String(v)} could not be cloned.`, 'DataCloneError');
    return v;
  }
  if (seen.has(v)) return seen.get(v);
  let out;
  if (Array.isArray(v)) {
    out = [];
    seen.set(v, out);
    for (let i = 0; i < v.length; i++) out[i] = clone(v[i], seen);
    return out;
  }
  if (v instanceof Date) return new Date(v.getTime());
  if (v instanceof RegExp) return new RegExp(v.source, v.flags);
  if (v instanceof Map) { out = new Map(); seen.set(v, out); for (const [a, b] of v) out.set(clone(a, seen), clone(b, seen)); return out; }
  if (v instanceof Set) { out = new Set(); seen.set(v, out); for (const a of v) out.add(clone(a, seen)); return out; }
  if (v instanceof ArrayBuffer) return v.slice(0);
  if (ArrayBuffer.isView(v)) return v.slice ? v.slice() : new DataView(v.buffer.slice(0));
  if (v instanceof Error) { const e = new v.constructor(v.message); seen.set(v, e); return e; }
  if (KEYS.has(v)) return clonaChave(v);
  if (v instanceof Node) throw new DOMException(`${Object.prototype.toString.call(v)} could not be cloned.`, 'DataCloneError');
  out = {};
  seen.set(v, out);
  for (const k of Object.keys(v)) out[k] = clone(v[k], seen);
  return out;
}
hidden(G, 'structuredClone', function structuredClone(v) { return clone(v, new Map()); });

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
// aqui ficam a normalização dos algoritmos, os CryptoKey (o material vive no
// WeakMap KEYS, fora do alcance da página), o JWK e os erros. A ordem das
// checagens e as mensagens seguem o Chrome (Blink + components/webcrypto).
const KEYS = new WeakMap();
let ATUAL = '';
const str = (v) => `${v}`;
const erroDe = (nome, msg) => (nome === 'TypeError' ? new TypeError(msg) : new DOMException(msg, nome));
// os erros que o Blink lança pelo ExceptionState levam o prefixo; os da operação, não
const ctxErro = (nome, msg) => erroDe(nome, `Failed to execute '${ATUAL}' on 'SubtleCrypto': ${msg}`);
const opErro = (nome, msg = '') => erroDe(nome, msg);
const juntar = (...partes) => partes.flat().join(': ');
const VAZIO = new Uint8Array(0);

const AB_LEN = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
const ehBuffer = (v) => { try { AB_LEN.call(v); return true; } catch { return false; } };
const ehBufferSource = (v) => ArrayBuffer.isView(v) || ehBuffer(v);
// cópia dos bytes (buffer desanexado vira vazio, como no Chrome)
function copia(v) {
  try {
    return ArrayBuffer.isView(v) ? new Uint8Array(v.buffer, v.byteOffset, v.byteLength).slice() : new Uint8Array(v).slice();
  } catch { return new Uint8Array(0); }
}
const iguais = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
// TruncateToBitLength: ceil(bits/8) bytes, bits que sobram zerados
function corta(b, bits) {
  const n = Math.ceil(bits / 8);
  const r = b.slice(0, n);
  if (bits % 8) r[n - 1] &= (0xff00 >> (bits % 8)) & 0xff;
  return r;
}

// conversões do WebIDL
function sequencia(v, ctx = '') {
  if (v === null || (typeof v !== 'object' && typeof v !== 'function') || typeof v[Symbol.iterator] !== 'function')
    throw ctxErro('TypeError', `${ctx}The provided value cannot be converted to a sequence.`);
  return Array.from(v, (x) => str(x));
}
const argAlg = (v) => (v !== null && (typeof v === 'object' || typeof v === 'function') ? v : str(v));
function argChave(v, i) {
  if (!KEYS.has(v)) throw ctxErro('TypeError', `parameter ${i} is not of type 'CryptoKey'.`);
}
function argBuffer(v) {
  if (!ehBufferSource(v)) throw ctxErro('TypeError', "The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
}
function uint32(v) {
  const n = +v;
  if (!Number.isFinite(n)) throw ctxErro('TypeError', `Value is${Number.isNaN(n) ? '' : ' infinite and'} not of type 'unsigned long'.`);
  const t = Math.trunc(n);
  if (t < 0 || t > 0xffffffff) throw ctxErro('TypeError', "Value is outside the 'unsigned long' value range.");
  return t;
}

// usos na ordem canônica do Chrome (é a ordem de CryptoKey.usages)
const USOS = ['encrypt', 'decrypt', 'sign', 'verify', 'deriveKey', 'deriveBits', 'wrapKey', 'unwrapKey'];
function validaUsos(lista) {
  if (!lista.every((u) => USOS.includes(u))) throw ctxErro('TypeError', 'Invalid keyUsages argument');
  return USOS.filter((u) => lista.includes(u));
}
const FORMATOS = ['raw', 'pkcs8', 'spki', 'jwk', 'raw-public', 'raw-private', 'raw-seed', 'raw-secret'];
function formato(f) {
  if (!FORMATOS.includes(f)) throw ctxErro('TypeError', `Invalid keyFormat argument: ${f}`);
}

// ---- normalização dos algoritmos (Blink: normalize_algorithm.cc)
const maiusc = (s) => s.replace(/[a-z]+/g, (c) => c.toUpperCase());
const POR_NOME = new Map(['AES-CBC', 'AES-CTR', 'AES-GCM', 'AES-KW', 'HMAC', 'RSASSA-PKCS1-v1_5', 'RSA-PSS', 'RSA-OAEP',
  'ECDSA', 'ECDH', 'Ed25519', 'X25519', 'HKDF', 'PBKDF2', 'SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'].map((n) => [maiusc(n), n]));
// por operação: algoritmo → dicionário dos parâmetros ('' = nenhum)
const OPS = (() => {
  const t = (nomes, p) => Object.fromEntries(nomes.map((n) => [n, p]));
  const AES = ['AES-CBC', 'AES-CTR', 'AES-GCM'], RSA = ['RSASSA-PKCS1-v1_5', 'RSA-PSS', 'RSA-OAEP'];
  const cifra = { 'AES-CBC': 'AesCbcParams', 'AES-CTR': 'AesCtrParams', 'AES-GCM': 'AeadParams', 'RSA-OAEP': 'RsaOaepParams' };
  const assina = { HMAC: '', 'RSASSA-PKCS1-v1_5': '', 'RSA-PSS': 'RsaPssParams', ECDSA: 'EcdsaParams', Ed25519: '' };
  return {
    encrypt: cifra, decrypt: cifra, sign: assina, verify: assina,
    digest: t(['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'], ''),
    generateKey: { ...t([...AES, 'AES-KW'], 'AesKeyGenParams'), HMAC: 'HmacKeyGenParams', ...t(RSA, 'RsaHashedKeyGenParams'),
      ...t(['ECDSA', 'ECDH'], 'EcKeyGenParams'), Ed25519: '', X25519: '' },
    importKey: { ...t([...AES, 'AES-KW', 'HKDF', 'PBKDF2', 'Ed25519', 'X25519'], ''), HMAC: 'HmacImportParams',
      ...t(RSA, 'RsaHashedImportParams'), ...t(['ECDSA', 'ECDH'], 'EcKeyImportParams') },
    'get key length': { ...t([...AES, 'AES-KW'], 'AesDerivedKeyParams'), HMAC: 'HmacImportParams', HKDF: '', PBKDF2: '' },
    deriveBits: { ECDH: 'EcdhKeyDeriveParams', X25519: 'EcdhKeyDeriveParams', HKDF: 'HkdfParams', PBKDF2: 'Pbkdf2Params' },
    wrapKey: { ...cifra, 'AES-KW': '' }, unwrapKey: { ...cifra, 'AES-KW': '' },
    exportKey: t([...AES, 'AES-KW', 'HMAC', ...RSA, 'ECDSA', 'ECDH', 'Ed25519', 'X25519'], ''),
  };
})();
// `ctx` é a pilha das mensagens ("HmacImportParams: hash: Algorithm: Unrecognized name")
function normaliza(alg, op, ctx = []) {
  let dic = {}, nome = alg;
  if (typeof alg !== 'string') {
    dic = alg;
    const v = alg.name;
    if (v === undefined) throw ctxErro('TypeError', juntar(ctx, 'Algorithm', 'name', 'Missing or not a string'));
    nome = str(v);
  }
  const canon = POR_NOME.get(maiusc(nome));
  if (!canon) throw ctxErro('NotSupportedError', juntar(ctx, 'Algorithm', 'Unrecognized name'));
  const tipo = OPS[op][canon];
  if (tipo === undefined) throw ctxErro('NotSupportedError', juntar(ctx, canon, 'Unsupported operation', op));
  const r = { name: canon };
  if (tipo) PARAMS[tipo](dic, [...ctx, tipo], r);
  return r;
}
function membroBuffer(d, k, c, obrigatorio) {
  const v = d[k];
  if (v === undefined) {
    if (obrigatorio) throw ctxErro('TypeError', juntar(c, k, 'Missing required property'));
    return undefined;
  }
  if (!ehBufferSource(v)) throw ctxErro('TypeError', juntar(c, k, 'Not a BufferSource'));
  return copia(v);
}
// inteiro com [EnforceRange]
function membroInteiro(d, k, c, max, obrigatorio) {
  const v = d[k];
  if (v === undefined) {
    if (obrigatorio) throw ctxErro('TypeError', juntar(c, k, 'Missing required property'));
    return undefined;
  }
  let n;
  try { n = +v; } catch { n = NaN; }
  if (Number.isNaN(n)) throw ctxErro('TypeError', juntar(c, k, 'Is not a number'));
  n = Math.trunc(n);
  if (!Number.isFinite(n) || n < 0 || n > max) throw ctxErro('TypeError', juntar(c, k, 'Outside of numeric range'));
  return n;
}
function membroHash(d, c) {
  let v = d.hash;
  if (v === null || (typeof v !== 'object' && typeof v !== 'function')) {
    if (v === undefined) throw ctxErro('TypeError', juntar(c, 'hash', 'Missing or not an AlgorithmIdentifier'));
    v = str(v);
  }
  return normaliza(v, 'digest', [...c, 'hash']).name;
}
function membroCurva(d, c) {
  const v = d.namedCurve;
  if (v === undefined) throw ctxErro('TypeError', juntar(c, 'namedCurve', 'Missing or not a string'));
  const s = str(v);
  if (s !== 'P-256' && s !== 'P-384' && s !== 'P-521') throw ctxErro('NotSupportedError', juntar(c, 'Unrecognized namedCurve'));
  return s;
}
const PARAMS = {
  AesCbcParams(d, c, r) { r.iv = membroBuffer(d, 'iv', c, true); },
  AesKeyGenParams(d, c, r) { r.length = membroInteiro(d, 'length', c, 0xffff, true); },
  AesDerivedKeyParams(d, c, r) { r.length = membroInteiro(d, 'length', c, 0xffff, true); },
  HmacImportParams(d, c, r) { r.hash = membroHash(d, c); r.length = membroInteiro(d, 'length', c, 0xffffffff, false); },
  HmacKeyGenParams(d, c, r) { r.hash = membroHash(d, c); r.length = membroInteiro(d, 'length', c, 0xffffffff, false); },
  RsaHashedKeyGenParams(d, c, r) {
    r.modulusLength = membroInteiro(d, 'modulusLength', c, 0xffffffff, true);
    const e = d.publicExponent;
    if (TA_TAG.call(e) !== 'Uint8Array') throw ctxErro('TypeError', juntar(c, 'publicExponent', 'Missing or not a Uint8Array'));
    r.publicExponent = e.length ? copia(e) : new Uint8Array(1);
    r.hash = membroHash(d, c);
  },
  RsaHashedImportParams(d, c, r) { r.hash = membroHash(d, c); },
  AesCtrParams(d, c, r) { r.counter = membroBuffer(d, 'counter', c, true); r.length = membroInteiro(d, 'length', c, 0xff, true); },
  AeadParams(d, c, r) {
    r.iv = membroBuffer(d, 'iv', c, true);
    r.additionalData = membroBuffer(d, 'additionalData', c, false);
    r.tagLength = membroInteiro(d, 'tagLength', c, 0xff, false);
  },
  RsaOaepParams(d, c, r) { r.label = membroBuffer(d, 'label', c, false); },
  RsaPssParams(d, c, r) { r.saltLength = membroInteiro(d, 'saltLength', c, 0xffffffff, true); },
  EcdsaParams(d, c, r) { r.hash = membroHash(d, c); },
  EcKeyGenParams(d, c, r) { r.namedCurve = membroCurva(d, c); },
  EcKeyImportParams(d, c, r) { r.namedCurve = membroCurva(d, c); },
  EcdhKeyDeriveParams(d, c, r) {
    const v = d.public;
    if (v === undefined) throw ctxErro('TypeError', juntar(c, 'public', 'Missing required property'));
    if (!KEYS.has(v)) throw ctxErro('TypeError', juntar(c, 'public', 'Must be a CryptoKey'));
    r.public = v;
  },
  Pbkdf2Params(d, c, r) {
    r.salt = membroBuffer(d, 'salt', c, true);
    r.iterations = membroInteiro(d, 'iterations', c, 0xffffffff, true);
    r.hash = membroHash(d, c);
  },
  HkdfParams(d, c, r) { r.hash = membroHash(d, c); r.salt = membroBuffer(d, 'salt', c, true); r.info = membroBuffer(d, 'info', c, true); },
};

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
const clonaChave = (k) => { const s = KEYS.get(k); return novaChave(s.tipo, s.ext, s.alg, s.usos, s.m); };
// a ordem das propriedades é a do Chrome: name, hash, length/modulusLength...
function algoritmoJs(a) {
  const o = { name: a.name };
  if (a.hash) o.hash = { name: a.hash };
  for (const k of ['length', 'modulusLength', 'namedCurve']) if (a[k] !== undefined) o[k] = a[k];
  if (a.publicExponent) o.publicExponent = a.publicExponent.slice();
  return o;
}
// CryptoKeyPair é um dicionário: membros em ordem lexicográfica
const parChaves = (pub, priv) => ({ privateKey: priv, publicKey: pub });
function podeUsar(k, alg, uso) {
  const s = KEYS.get(k);
  if (s.alg.name !== alg.name) throw opErro('InvalidAccessError', 'key.algorithm does not match that of operation');
  if (!s.usos.includes(uso)) throw opErro('InvalidAccessError', 'key.usages does not permit this operation');
  return s;
}
function checaUsos(todos, usos) {
  if (usos.some((u) => !todos.includes(u))) throw opErro('SyntaxError', 'Cannot create a key using the specified key usages.');
}
function semUsos(usos) {
  if (!usos.length) throw opErro('SyntaxError', 'Usages cannot be empty when creating a key.');
}
const formatoImport = () => opErro('NotSupportedError', 'Unsupported import key format for algorithm');
const formatoExport = () => opErro('NotSupportedError', 'Unsupported export key format for algorithm');
function tipoChave(s, tipo) {
  if (s.tipo !== tipo) throw opErro('InvalidAccessError', 'The key is not of the expected type');
}

// ---- JWK (o JwkReader/JwkWriter do Chrome)
const b64u = (b) => b.toBase64({ alphabet: 'base64url', omitPadding: true });
function de64u(s) {
  if (!/^[A-Za-z0-9_-]*$/.test(s) || s.length % 4 === 1) return null;
  try { return Uint8Array.fromBase64(s, { alphabet: 'base64url', lastChunkHandling: 'loose' }); } catch { return null; }
}
// o dicionário JsonWebKey do importKey, com os membros convertidos pelo WebIDL
function jwkDicionario(v) {
  if (v !== null && v !== undefined && typeof v !== 'object' && typeof v !== 'function')
    throw ctxErro('TypeError', "The provided value is not of type '(ArrayBuffer or ArrayBufferView or JsonWebKey)'.");
  const r = {};
  if (v === null || v === undefined) return r;
  for (const k of ['alg', 'crv', 'd', 'dp', 'dq', 'e', 'ext', 'k', 'key_ops', 'kty', 'n', 'p', 'priv', 'pub', 'q', 'qi', 'use', 'x', 'y']) {
    const x = v[k];
    if (x === undefined) continue;
    if (k === 'ext') r.ext = !!x;
    else if (k === 'key_ops') r.key_ops = sequencia(x, "Failed to read the 'key_ops' property from 'JsonWebKey': ");
    else r[k] = str(x);
  }
  return r;
}
const USO_ENC = ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey', 'deriveKey', 'deriveBits'];
const jwkErro = (msg) => opErro('DataError', msg);
const jwkTipo = (nome, tipo) => jwkErro(`The JWK member "${nome}" must be a ${tipo}`);
const tem = (j, k) => Object.prototype.hasOwnProperty.call(j, k);
function jwkOpcional(j, nome, tipo) {
  if (!tem(j, nome)) return undefined;
  if (typeof j[nome] !== tipo) throw jwkTipo(nome, tipo);
  return j[nome];
}
function jwkTexto(j, nome) {
  if (!tem(j, nome)) throw jwkErro(`The required JWK member "${nome}" was missing`);
  if (typeof j[nome] !== 'string') throw jwkTipo(nome, 'string');
  return j[nome];
}
function jwkBytes(j, nome) {
  const b = de64u(jwkTexto(j, nome));
  if (!b) throw jwkErro(`The JWK member "${nome}" could not be base64url decoded or contained padding`);
  return b;
}
function jwkInteiro(j, nome) {
  const b = jwkBytes(j, nome);
  if (!b.length) throw jwkErro(`The JWK "${nome}" member was empty.`);
  if (b.length > 1 && !b[0]) throw jwkErro(`The JWK "${nome}" member contained a leading zero.`);
  return b;
}
function jwkTamanho(j, nome, n) {
  const b = jwkBytes(j, nome);
  if (b.length !== n) throw jwkErro(`The JWK's "${nome}" member defines an octet string of length ${b.length} bytes but should be ${n}`);
  return b;
}
function jwkAlg(j, esperado) {
  const a = jwkOpcional(j, 'alg', 'string');
  if (a !== undefined && a !== esperado) throw jwkErro('The JWK "alg" member was inconsistent with that specified by the Web Crypto call');
}
// JwkReader::Init: kty, ext, key_ops/use e alg; no unwrapKey o JWK chega como texto JSON
function jwkInicia(j, ext, usos, kty, alg) {
  if (j instanceof Uint8Array) {
    try { j = JSON.parse(N.utf8Decode(j)); } catch { j = null; }
    if (j === null || typeof j !== 'object' || Array.isArray(j)) throw jwkErro('JWK input could not be parsed to a JSON dictionary');
  }
  const k = jwkTexto(j, 'kty');
  if (k !== kty) throw jwkErro(`The JWK "kty" member was not "${kty}"`);
  if (jwkOpcional(j, 'ext', 'boolean') === false && ext)
    throw jwkErro('The "ext" member of the JWK dictionary is inconsistent what that specified by the Web Crypto call');
  let ops;
  if (tem(j, 'key_ops')) {
    if (!Array.isArray(j.key_ops)) throw jwkTipo('key_ops', 'list');
    ops = [];
    j.key_ops.forEach((op, i) => {
      if (typeof op !== 'string') throw jwkTipo(`key_ops[${i}]`, 'string');
      if (j.key_ops.indexOf(op) !== i) throw jwkErro('The "key_ops" member of the JWK dictionary contains duplicate usages.');
      if (USOS.includes(op)) ops.push(op);
    });
    if (usos.some((u) => !ops.includes(u)))
      throw jwkErro('The JWK "key_ops" member was inconsistent with that specified by the Web Crypto call. The JWK usage must be a superset of those requested');
  }
  const use = jwkOpcional(j, 'use', 'string');
  if (use !== undefined) {
    const m = use === 'enc' ? USO_ENC : use === 'sig' ? ['sign', 'verify'] : null;
    if (!m) throw jwkErro('The JWK "use" member could not be parsed');
    if (usos.some((u) => !m.includes(u)))
      throw jwkErro('The JWK "use" member was inconsistent with that specified by the Web Crypto call. The JWK usage must be a superset of those requested');
    if (ops && ops.some((u) => !m.includes(u)))
      throw jwkErro('The JWK "use" and "key_ops" properties were both found but are inconsistent with each other.');
  }
  if (alg) jwkAlg(j, alg);
  return j;
}
// membros em ordem lexicográfica, como no dicionário JsonWebKey do Chrome
function jwkSaida(s, campos) {
  const o = { ...campos, ext: s.ext, key_ops: s.usos.slice() };
  const r = {};
  for (const k of Object.keys(o).sort()) if (o[k] !== undefined) r[k] = o[k];
  return r;
}

// ---- algoritmos: as chaves secretas guardam os bytes; EC {d, pub}; RSA
// [n, e(, d, p, q, dp, dq, qi)]; Ed25519/X25519 {priv, pub}
const U_AES = ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'];
const U_KDF = ['deriveKey', 'deriveBits'];
const BLOCO = { 'SHA-1': 512, 'SHA-256': 512, 'SHA-384': 1024, 'SHA-512': 1024 };
const bruto = (f) => f === 'raw' || f === 'raw-secret';
function aes(sufixo, todos, extra) {
  const nomeJwk = (n) => (n === 16 || n === 24 || n === 32 ? `A${n * 8}${sufixo}` : '');
  function importaBruta(b, alg, ext, usos) {
    checaUsos(todos, usos);
    if (b.length === 24) throw opErro('OperationError', '192-bit AES keys are not supported');
    if (b.length !== 16 && b.length !== 32) throw opErro('DataError', 'AES key data must be 128 or 256 bits');
    return novaChave('secret', ext, { name: alg.name, length: b.length * 8 }, usos, b);
  }
  function tamanho(n, erro) {
    if (n === 192) throw opErro('OperationError', '192-bit AES keys are not supported');
    if (n !== 128 && n !== 256) throw opErro('OperationError', erro);
  }
  return {
    generate(alg, ext, usos) {
      checaUsos(todos, usos);
      tamanho(alg.length, 'AES key length must be 128 or 256 bits');
      semUsos(usos);
      return novaChave('secret', ext, { name: alg.name, length: alg.length }, usos, N.cryptoRandom(alg.length / 8));
    },
    import(fmt, dado, alg, ext, usos) {
      if (bruto(fmt)) return importaBruta(dado, alg, ext, usos);
      if (fmt !== 'jwk') throw formatoImport();
      checaUsos(todos, usos);
      const j = jwkInicia(dado, ext, usos, 'oct');
      const k = jwkBytes(j, 'k');
      const a = jwkOpcional(j, 'alg', 'string');
      if (a !== undefined && a !== nomeJwk(k.length)) {
        if ([16, 24, 32].some((n) => a === nomeJwk(n)))
          throw jwkErro('The JWK "k" member did not include the right length of key data for the given algorithm.');
        throw jwkErro('The JWK "alg" member was inconsistent with that specified by the Web Crypto call');
      }
      return importaBruta(k, alg, ext, usos);
    },
    export(fmt, s) {
      if (bruto(fmt)) return s.m.slice();
      if (fmt !== 'jwk') throw formatoExport();
      return jwkSaida(s, { alg: nomeJwk(s.m.length), k: b64u(s.m), kty: 'oct' });
    },
    keyLength(alg) {
      tamanho(alg.length, 'AES key length must be 128 or 256 bits');
      return alg.length;
    },
    ...extra,
  };
}
function hmacJwk(hash) { return `HS${hash.slice(4)}`; }
const HMAC = {
  generate(alg, ext, usos) {
    checaUsos(['sign', 'verify'], usos);
    let bits = alg.length;
    if (bits === undefined) bits = BLOCO[alg.hash];
    else if (!bits) throw opErro('OperationError', 'HMAC key length must not be zero');
    semUsos(usos);
    const m = corta(N.cryptoRandom(Math.ceil(bits / 8)), bits);
    return novaChave('secret', ext, { name: 'HMAC', hash: alg.hash, length: bits }, usos, m);
  },
  import(fmt, dado, alg, ext, usos) {
    if (fmt === 'jwk') {
      checaUsos(['sign', 'verify'], usos);
      const j = jwkInicia(dado, ext, usos, 'oct');
      dado = jwkBytes(j, 'k');
      jwkAlg(j, hmacJwk(alg.hash));
    } else if (!bruto(fmt)) throw formatoImport();
    checaUsos(['sign', 'verify'], usos);
    if (!dado.length) throw opErro('DataError', 'HMAC key data must not be empty');
    let bits = dado.length * 8;
    if (alg.length !== undefined) {
      if (Math.ceil(alg.length / 8) !== dado.length)
        throw opErro('DataError', 'The optional HMAC key length must be shorter than the key data, and by no more than 7 bits.');
      bits = alg.length;
    }
    return novaChave('secret', ext, { name: 'HMAC', hash: alg.hash, length: bits }, usos, corta(dado, bits));
  },
  export(fmt, s) {
    if (bruto(fmt)) return s.m.slice();
    if (fmt !== 'jwk') throw formatoExport();
    return jwkSaida(s, { alg: hmacJwk(s.alg.hash), k: b64u(s.m), kty: 'oct' });
  },
  sign: (a, s, d) => N.cryptoHmac(s.alg.hash, s.m, d),
  verify: (a, s, sig, d) => N.cryptoHmac(s.alg.hash, s.m, d, sig),
  keyLength(alg) {
    if (alg.length === undefined) return BLOCO[alg.hash];
    if (!alg.length) throw opErro('TypeError', 'HMAC key length must not be zero');
    return alg.length;
  },
};
// PBKDF2 e HKDF: só "raw", nunca extraíveis
function kdf(nome, derivar) {
  return {
    import(fmt, dado, alg, ext, usos) {
      if (!bruto(fmt)) throw formatoImport();
      checaUsos(U_KDF, usos);
      if (ext) throw opErro('SyntaxError', 'KDF keys must set extractable=false');
      return novaChave('secret', false, { name: nome }, usos, dado);
    },
    keyLength: () => null,
    deriveBits: derivar,
  };
}
const PBKDF2 = kdf('PBKDF2', (a, s, n) => {
  if (n === null) throw opErro('OperationError', 'No length was specified for the PBKDF2 Derive Bits operation.');
  if (n % 8) throw opErro('OperationError', 'Length for PBKDF2 key derivation must be a multiple of 8 bits.');
  if (!a.iterations) throw opErro('OperationError', 'PBKDF2 requires iterations > 0');
  return N.cryptoPbkdf2(a.hash, s.m, a.salt, a.iterations, n / 8);
});
const HKDF = kdf('HKDF', (a, s, n) => {
  if (n === null) throw opErro('OperationError', 'No length was specified for the HKDF Derive Bits operation.');
  if (n % 8) throw opErro('OperationError', 'The length provided for HKDF is not a multiple of 8 bits.');
  return N.cryptoHkdf(a.hash, s.m, a.salt, a.info, n / 8);
});
// ECDSA e ECDH (P-256 e P-384; a P-521 é reconhecida mas não implementada)
function ec(nome, pub, priv, algJwk, extra) {
  const todos = [...pub, ...priv];
  function curva(c) {
    if (c === 'P-521') throw opErro('NotSupportedError', 'The requested operation is unsupported');
    return c;
  }
  const alg = (c) => ({ name: nome, namedCurve: c });
  const publica = (c, ext, usos, p) => novaChave('public', ext, alg(c), usos, { d: null, pub: p });
  const privada = (c, ext, usos, d, p) => novaChave('private', ext, alg(c), usos, { d, pub: p });
  return {
    generate(a, ext, usos) {
      checaUsos(todos, usos);
      const c = curva(a.namedCurve);
      const uv = usos.filter((u) => priv.includes(u));
      semUsos(uv);
      const [d, p] = N.cryptoEc('gerar', c);
      return parChaves(publica(c, true, usos.filter((u) => pub.includes(u)), p), privada(c, ext, uv, d, p));
    },
    import(fmt, dado, a, ext, usos) {
      const c = a.namedCurve;
      if (fmt === 'raw' || fmt === 'raw-public' || fmt === 'spki') {
        checaUsos(pub, usos);
        return publica(c, ext, usos, N.cryptoEc(fmt === 'spki' ? 'spki' : 'raw', curva(c), dado));
      }
      if (fmt === 'pkcs8') {
        checaUsos(priv, usos);
        const [d, p] = N.cryptoEc('pkcs8', curva(c), dado);
        return privada(c, ext, usos, d, p);
      }
      if (fmt !== 'jwk') throw formatoImport();
      const j = jwkInicia(dado, ext, usos, 'EC', algJwk && algJwk[c]);
      if (jwkTexto(j, 'crv') !== c) throw jwkErro('The JWK\'s "crv" member specifies a different curve than requested');
      const ehPrivada = tem(j, 'd');
      checaUsos(ehPrivada ? priv : pub, usos);
      curva(c);
      const n = c === 'P-256' ? 32 : 48;
      const ponto = new Uint8Array(2 * n + 1);
      ponto[0] = 4;
      ponto.set(jwkTamanho(j, 'x', n), 1);
      ponto.set(jwkTamanho(j, 'y', n), n + 1);
      const p = N.cryptoEc('raw', c, ponto);
      if (!ehPrivada) return publica(c, ext, usos, p);
      const [d] = N.cryptoEc('jwk', c, ponto.subarray(1), jwkTamanho(j, 'd', n));
      return privada(c, ext, usos, d, p);
    },
    export(fmt, s) {
      const c = s.alg.namedCurve;
      if (fmt === 'raw' || fmt === 'raw-public') { tipoChave(s, 'public'); return s.m.pub.slice(); }
      if (fmt === 'spki') { tipoChave(s, 'public'); return N.cryptoEc('para-spki', c, s.m.pub); }
      if (fmt === 'pkcs8') { tipoChave(s, 'private'); return N.cryptoEc('para-pkcs8', c, s.m.d); }
      if (fmt !== 'jwk') throw formatoExport();
      const n = (s.m.pub.length - 1) / 2;
      return jwkSaida(s, { crv: c, d: s.m.d ? b64u(s.m.d) : undefined, kty: 'EC',
        x: b64u(s.m.pub.subarray(1, n + 1)), y: b64u(s.m.pub.subarray(n + 1)) });
    },
    ...extra,
  };
}
const ECDSA = ec('ECDSA', ['verify'], ['sign'], { 'P-256': 'ES256', 'P-384': 'ES384', 'P-521': 'ES512' }, {
  sign(a, s, d) { tipoChave(s, 'private'); return N.cryptoEc('assina', s.alg.namedCurve, a.hash, s.m.d, d); },
  verify(a, s, sig, d) { tipoChave(s, 'public'); return N.cryptoEc('verifica', s.alg.namedCurve, a.hash, s.m.pub, d, sig); },
});
const ECDH = ec('ECDH', [], U_KDF, null, {
  deriveBits(a, s, n) {
    tipoChave(s, 'private');
    const p = KEYS.get(a.public);
    if (p.tipo !== 'public' || !p.alg.namedCurve)
      throw opErro('InvalidAccessError', 'The public parameter for ECDH key derivation is not a public EC key');
    if (p.alg.name !== 'ECDH') throw opErro('InvalidAccessError', 'The public parameter for ECDH key derivation must be for ECDH');
    const c = s.alg.namedCurve;
    if (p.alg.namedCurve !== c)
      throw opErro('InvalidAccessError', 'The public parameter for ECDH key derivation is for a different named curve');
    const max = c === 'P-256' ? 256 : 384;
    if (n === null) n = max;
    if (!n) return new Uint8Array(0);
    if (n > max) throw opErro('OperationError', `Length specified for ECDH key derivation is too large. Maximum allowed is ${max} bits`);
    return corta(N.cryptoEc('ecdh', c, s.m.d, p.m.pub), n);
  },
});
// RSASSA-PKCS1-v1_5, RSA-PSS e RSA-OAEP
const bitsDe = (n) => (n.length - 1) * 8 + 32 - Math.clz32(n[0]);
function expoente(b) {
  let v = 0;
  for (const x of b) if ((v = v * 256 + x) > 0xffffffff) return -1;
  return v;
}
function rsa(nome, pub, priv, jwkAlgs, extra) {
  const todos = [...pub, ...priv];
  const alg = (h, c) => ({ name: nome, hash: h, modulusLength: bitsDe(c[0]), publicExponent: c[1].slice() });
  const publica = (h, ext, usos, c) => novaChave('public', ext, alg(h, c), usos, c.slice(0, 2));
  const privada = (h, ext, usos, c) => novaChave('private', ext, alg(h, c), usos, c);
  return {
    generate(a, ext, usos) {
      checaUsos(todos, usos);
      const bits = a.modulusLength;
      if (bits < 256 || bits > 8192 || bits % 8)
        throw opErro('OperationError', 'The modulus length must be a multiple of 8 bits and >= 256 and <= 8192');
      const e = expoente(a.publicExponent);
      if (e !== 3 && e !== 65537) throw opErro('OperationError', 'The "publicExponent" must be either 3 or 65537');
      const uv = usos.filter((u) => priv.includes(u));
      semUsos(uv);
      const c = N.cryptoRsa('gerar', bits, e);
      return parChaves(publica(a.hash, true, usos.filter((u) => pub.includes(u)), c), privada(a.hash, ext, uv, c));
    },
    import(fmt, dado, a, ext, usos) {
      if (fmt === 'spki') { checaUsos(pub, usos); return publica(a.hash, ext, usos, N.cryptoRsa('spki', dado)); }
      if (fmt === 'pkcs8') { checaUsos(priv, usos); return privada(a.hash, ext, usos, N.cryptoRsa('pkcs8', dado)); }
      if (fmt !== 'jwk') throw formatoImport();
      const j = jwkInicia(dado, ext, usos, 'RSA', jwkAlgs[a.hash]);
      const c = [jwkInteiro(j, 'n'), jwkInteiro(j, 'e')];
      const ehPrivada = tem(j, 'd');
      if (ehPrivada) for (const k of ['d', 'p', 'q', 'dp', 'dq', 'qi']) c.push(jwkInteiro(j, k));
      checaUsos(ehPrivada ? priv : pub, usos);
      N.cryptoRsa('jwk', c);
      return (ehPrivada ? privada : publica)(a.hash, ext, usos, c);
    },
    export(fmt, s) {
      if (fmt === 'spki') { tipoChave(s, 'public'); return N.cryptoRsa('para-spki', s.m); }
      if (fmt === 'pkcs8') { tipoChave(s, 'private'); return N.cryptoRsa('para-pkcs8', s.m); }
      if (fmt !== 'jwk') throw formatoExport();
      const campos = { alg: jwkAlgs[s.alg.hash], kty: 'RSA' };
      s.m.forEach((b, i) => { campos[['n', 'e', 'd', 'p', 'q', 'dp', 'dq', 'qi'][i]] = b64u(b); });
      return jwkSaida(s, campos);
    },
    ...extra,
  };
}
const algsJwk = (f) => ({ 'SHA-1': f('1'), 'SHA-256': f('256'), 'SHA-384': f('384'), 'SHA-512': f('512') });
function assinaturaRsa(esquema) {
  const sal = (a) => (esquema === 'pss' ? a.saltLength : 0);
  return {
    sign(a, s, d) { tipoChave(s, 'private'); return N.cryptoRsa('assina', s.m, esquema, s.alg.hash, d, sal(a)); },
    verify(a, s, sig, d) { tipoChave(s, 'public'); return N.cryptoRsa('verifica', s.m, esquema, s.alg.hash, d, sal(a), sig); },
  };
}
const RSASSA = rsa('RSASSA-PKCS1-v1_5', ['verify'], ['sign'], algsJwk((h) => `RS${h}`), assinaturaRsa('pkcs1'));
const RSA_PSS = rsa('RSA-PSS', ['verify'], ['sign'], algsJwk((h) => `PS${h}`), assinaturaRsa('pss'));
const RSA_OAEP = rsa('RSA-OAEP', ['encrypt', 'wrapKey'], ['decrypt', 'unwrapKey'],
  algsJwk((h) => (h === '1' ? 'RSA-OAEP' : `RSA-OAEP-${h}`)), {
    encrypt(a, s, d) { tipoChave(s, 'public'); return N.cryptoRsa('oaep-e', s.m, s.alg.hash, a.label || VAZIO, d); },
    decrypt(a, s, d) { tipoChave(s, 'private'); return N.cryptoRsa('oaep-d', s.m, s.alg.hash, a.label || VAZIO, d); },
  });
// Ed25519 e X25519
function okp(nome, pub, priv, extra) {
  const todos = [...pub, ...priv];
  const publica = (ext, usos, p) => novaChave('public', ext, { name: nome }, usos, { priv: null, pub: p });
  const privada = (ext, usos, s) => novaChave('private', ext, { name: nome }, usos, { priv: s, pub: N.cryptoOkp('publica', nome, s) });
  return {
    generate(a, ext, usos) {
      checaUsos(todos, usos);
      const uv = usos.filter((u) => priv.includes(u));
      semUsos(uv);
      const k = privada(ext, uv, N.cryptoOkp('gerar', nome));
      return parChaves(publica(true, usos.filter((u) => pub.includes(u)), KEYS.get(k).m.pub), k);
    },
    import(fmt, dado, a, ext, usos) {
      if (fmt === 'raw' || fmt === 'raw-public') {
        checaUsos(pub, usos);
        if (dado.length !== 32) throw opErro('DataError', `${nome} key data must be 256 bits`);
        return publica(ext, usos, dado);
      }
      if (fmt === 'spki') { checaUsos(pub, usos); return publica(ext, usos, N.cryptoOkp('spki', nome, dado)); }
      if (fmt === 'pkcs8') { checaUsos(priv, usos); return privada(ext, usos, N.cryptoOkp('pkcs8', nome, dado)); }
      if (fmt !== 'jwk') throw formatoImport();
      const j = jwkInicia(dado, ext, usos, 'OKP');
      if (jwkTexto(j, 'crv') !== nome) throw jwkErro('The JWK\'s "crv" member specifies a different curve than requested');
      if (nome === 'Ed25519') {
        const al = jwkOpcional(j, 'alg', 'string');
        if (al !== undefined && al !== 'EdDSA' && al !== 'Ed25519')
          throw jwkErro('The JWK "alg" member was inconsistent with that specified by the Web Crypto call');
      }
      const ehPrivada = tem(j, 'd');
      checaUsos(ehPrivada ? priv : pub, usos);
      const x = jwkTamanho(j, 'x', 32);
      if (!ehPrivada) return publica(ext, usos, x);
      const k = privada(ext, usos, jwkTamanho(j, 'd', 32));
      if (!iguais(KEYS.get(k).m.pub, x)) throw opErro('DataError');
      return k;
    },
    export(fmt, s) {
      if (fmt === 'raw' || fmt === 'raw-public') { tipoChave(s, 'public'); return s.m.pub.slice(); }
      if (fmt === 'spki') { tipoChave(s, 'public'); return N.cryptoOkp('para-spki', nome, s.m.pub); }
      if (fmt === 'pkcs8') { tipoChave(s, 'private'); return N.cryptoOkp('para-pkcs8', nome, s.m.priv); }
      if (fmt !== 'jwk') throw formatoExport();
      return jwkSaida(s, { alg: nome === 'Ed25519' ? nome : undefined, crv: nome, d: s.m.priv ? b64u(s.m.priv) : undefined,
        kty: 'OKP', x: b64u(s.m.pub) });
    },
    ...extra,
  };
}
const ED25519 = okp('Ed25519', ['verify'], ['sign'], {
  sign(a, s, d) { tipoChave(s, 'private'); return N.cryptoOkp('assina', 'Ed25519', s.m.priv, d); },
  verify(a, s, sig, d) { tipoChave(s, 'public'); return N.cryptoOkp('verifica', 'Ed25519', s.m.pub, d, sig); },
});
const X25519 = okp('X25519', [], U_KDF, {
  deriveBits(a, s, n) {
    tipoChave(s, 'private');
    const p = KEYS.get(a.public);
    if (p.tipo !== 'public') throw opErro('InvalidAccessError', 'The public parameter for X25519 key derivation is not a public key');
    if (p.alg.name !== 'X25519') throw opErro('InvalidAccessError', 'The public parameter for X25519 key derivation must be for X25519');
    const b = N.cryptoOkp('deriva', 'X25519', s.m.priv, p.m.pub);
    if (n === null) return b;
    if (n > 256) throw opErro('OperationError', 'The length provided for X25519 is too large.');
    return corta(b, n);
  },
});
const IMPL = {
  'AES-CBC': aes('CBC', U_AES, {
    encrypt: (a, s, d) => N.cryptoAes('cbc-e', s.m, d, a.iv),
    decrypt: (a, s, d) => N.cryptoAes('cbc-d', s.m, d, a.iv),
  }),
  'AES-CTR': aes('CTR', U_AES, {
    encrypt: (a, s, d) => N.cryptoAes('ctr', s.m, d, a.counter, a.length),
    decrypt: (a, s, d) => N.cryptoAes('ctr', s.m, d, a.counter, a.length),
  }),
  'AES-GCM': aes('GCM', U_AES, {
    encrypt: (a, s, d) => N.cryptoAes('gcm-e', s.m, d, a.iv, a.additionalData || VAZIO, a.tagLength ?? 128),
    decrypt: (a, s, d) => N.cryptoAes('gcm-d', s.m, d, a.iv, a.additionalData || VAZIO, a.tagLength ?? 128),
  }),
  // no AES-KW, "cifrar" é o wrapKey e "decifrar" é o unwrapKey
  'AES-KW': aes('KW', ['wrapKey', 'unwrapKey'], {
    encrypt: (a, s, d) => N.cryptoAes('kw-e', s.m, d),
    decrypt: (a, s, d) => N.cryptoAes('kw-d', s.m, d),
  }),
  HMAC, 'RSASSA-PKCS1-v1_5': RSASSA, 'RSA-PSS': RSA_PSS, 'RSA-OAEP': RSA_OAEP, ECDSA, ECDH,
  Ed25519: ED25519, X25519, HKDF, PBKDF2,
};
// importKey (e o fim de unwrapKey/deriveKey): chave secreta ou privada sem usos não vale
function criaChave(fmt, dado, alg, ext, usos) {
  const k = IMPL[alg.name].import(fmt, dado, alg, ext, usos);
  const s = KEYS.get(k);
  if (s.tipo !== 'public') semUsos(s.usos);
  return k;
}

// ---- SubtleCrypto: os métodos validam os argumentos como o WebIDL e
// devolvem sempre uma Promise (os erros viram rejeição)
const saida = (b) => b.buffer;
function cifraOuDecifra(op, alg, k, d) {
  alg = argAlg(alg); argChave(k, 2); argBuffer(d);
  const a = normaliza(alg, op);
  const b = copia(d);
  return saida(IMPL[a.name][op](a, podeUsar(k, a, op), b));
}
const METODOS = {
  encrypt: [3, (alg, k, d) => cifraOuDecifra('encrypt', alg, k, d)],
  decrypt: [3, (alg, k, d) => cifraOuDecifra('decrypt', alg, k, d)],
  sign: [3, (alg, k, d) => {
    alg = argAlg(alg); argChave(k, 2); argBuffer(d);
    const a = normaliza(alg, 'sign');
    const b = copia(d);
    return saida(IMPL[a.name].sign(a, podeUsar(k, a, 'sign'), b));
  }],
  verify: [4, (alg, k, sig, d) => {
    alg = argAlg(alg); argChave(k, 2); argBuffer(sig); argBuffer(d);
    const a = normaliza(alg, 'verify');
    const bs = copia(sig), bd = copia(d);
    return IMPL[a.name].verify(a, podeUsar(k, a, 'verify'), bs, bd);
  }],
  digest: [2, (alg, d) => {
    alg = argAlg(alg); argBuffer(d);
    const a = normaliza(alg, 'digest');
    return saida(N.cryptoDigest(a.name, copia(d)));
  }],
  generateKey: [3, (alg, ext, usos) => {
    alg = argAlg(alg); ext = !!ext;
    const u = validaUsos(sequencia(usos));
    const a = normaliza(alg, 'generateKey');
    return IMPL[a.name].generate(a, ext, u);
  }],
  deriveKey: [5, (alg, k, tipo, ext, usos) => {
    alg = argAlg(alg); argChave(k, 2); tipo = argAlg(tipo); ext = !!ext;
    const u = validaUsos(sequencia(usos));
    const a = normaliza(alg, 'deriveBits');
    const ai = normaliza(tipo, 'importKey');
    const al = normaliza(tipo, 'get key length');
    const s = podeUsar(k, a, 'deriveKey');
    const bits = IMPL[a.name].deriveBits(a, s, IMPL[al.name].keyLength(al));
    return criaChave('raw-secret', bits, ai, ext, u);
  }],
  deriveBits: [2, (alg, k, n) => {
    alg = argAlg(alg); argChave(k, 2);
    n = n === undefined || n === null ? null : uint32(n);
    const a = normaliza(alg, 'deriveBits');
    return saida(IMPL[a.name].deriveBits(a, podeUsar(k, a, 'deriveBits'), n));
  }],
  importKey: [5, (fmt, dado, alg, ext, usos) => {
    fmt = str(fmt);
    const jwk = ehBufferSource(dado) ? null : jwkDicionario(dado);
    alg = argAlg(alg); ext = !!ext;
    const lista = sequencia(usos);
    formato(fmt);
    const u = validaUsos(lista);
    const a = normaliza(alg, 'importKey');
    if (fmt === 'jwk' && !jwk) throw ctxErro('TypeError', 'Key data must be an object for JWK import');
    if (fmt !== 'jwk' && jwk) throw ctxErro('TypeError', 'Key data must be a BufferSource for non-JWK formats');
    return criaChave(fmt, jwk || copia(dado), a, ext, u);
  }],
  exportKey: [2, (fmt, k) => {
    fmt = str(fmt); argChave(k, 2);
    formato(fmt);
    const s = KEYS.get(k);
    if (!(s.alg.name in OPS.exportKey)) throw ctxErro('NotSupportedError', `${s.alg.name}: Unsupported operation: exportKey`);
    if (!s.ext) throw ctxErro('InvalidAccessError', 'key is not extractable');
    const r = IMPL[s.alg.name].export(fmt, s);
    return r instanceof Uint8Array ? saida(r) : r;
  }],
  wrapKey: [4, (fmt, k, kw, alg) => {
    fmt = str(fmt); argChave(k, 2); argChave(kw, 3); alg = argAlg(alg);
    formato(fmt);
    const a = normaliza(alg, 'wrapKey');
    const sw = podeUsar(kw, a, 'wrapKey');
    const s = KEYS.get(k);
    if (!(s.alg.name in OPS.exportKey)) throw opErro('NotSupportedError', `${s.alg.name}: Unsupported operation: exportKey`);
    if (!s.ext) throw opErro('InvalidAccessError', 'key is not extractable');
    let b = IMPL[s.alg.name].export(fmt, s);
    if (!(b instanceof Uint8Array)) b = N.utf8Encode(JSON.stringify(b));
    return saida(IMPL[a.name].encrypt(a, sw, b));
  }],
  unwrapKey: [7, (fmt, w, kw, alg, algChave, ext, usos) => {
    fmt = str(fmt); argBuffer(w); argChave(kw, 3); alg = argAlg(alg); algChave = argAlg(algChave); ext = !!ext;
    const lista = sequencia(usos);
    formato(fmt);
    const u = validaUsos(lista);
    const a = normaliza(alg, 'unwrapKey');
    const ak = normaliza(algChave, 'importKey');
    const b = copia(w);
    const claro = IMPL[a.name].decrypt(a, podeUsar(kw, a, 'unwrapKey'), b);
    return criaChave(fmt, claro, ak, ext, u);
  }],
};
class SubtleCrypto {
  constructor() { throw new TypeError('Illegal constructor'); }
}
const SUBTLE = Object.create(SubtleCrypto.prototype);
for (const [nome, [minimo, corpo]] of Object.entries(METODOS)) {
  const f = { [nome](...args) {
    const antes = ATUAL;
    ATUAL = nome;
    try {
      if (this !== SUBTLE) throw new TypeError('Illegal invocation');
      if (args.length < minimo)
        throw ctxErro('TypeError', `${minimo} argument${minimo > 1 ? 's' : ''} required, but only ${args.length} present.`);
      return Promise.resolve(corpo(...args));
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
  static createObjectURL(blob) {
    const id = uuid();
    BLOBS.set(id, blob);
    return `blob:${G.origin}/${id}`;
  }
  static revokeObjectURL(u) { BLOBS.delete(String(u).split('/').pop()); }
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
      const b = BLOBS.get(req.url.split('/').pop());
      if (b) resolve(new Response(b, { status: 200 }));
      else reject(new TypeError('Failed to fetch'));
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
class MessagePort extends EventTarget {
  constructor() { super(); this._other = null; this._started = false; }
  postMessage(data) {
    const target = this._other;
    if (!target) return;
    const ev = new MessageEvent('message', { data: structuredClone(data) });
    setTimeout(() => fireTrusted(target, ev), 0);
  }
  start() {} close() { this._other = null; }
}
defineHandlers(MessagePort.prototype, ['message', 'messageerror']);
class MessageChannel {
  constructor() {
    this.port1 = new MessagePort();
    this.port2 = new MessagePort();
    this.port1._other = this.port2;
    this.port2._other = this.port1;
  }
}
hidden(G, 'MessagePort', MessagePort);
hidden(G, 'MessageChannel', MessageChannel);
class BroadcastChannel extends EventTarget {
  constructor(name) { super(); this.name = String(name); }
  postMessage() {} close() {}
}
defineHandlers(BroadcastChannel.prototype, ['message', 'messageerror']);
hidden(G, 'BroadcastChannel', BroadcastChannel);

// ---------------------------------------------------- ganchos do Rust
// (a página acrescenta os seus e publica em __ferroHooks no fim)
const hooks = {
  // módulos ES (ver script/module.rs)
  newNamespace() {
    const ns = Object.create(null);
    Object.defineProperty(ns, Symbol.toStringTag, { value: 'Module' });
    return ns;
  },
  importMeta(url) { return { url, resolve: (s) => new URL(s, url).href }; },
  moduleDone(p, ns) { return Promise.resolve(p).then(() => ns); },
};
// usados pelo código que o motor gera para os módulos
hidden(G, '__ferroExport', (ns, name, get) => Object.defineProperty(ns, name, { get, enumerable: true }));
hidden(G, '__ferroExportAll', (ns, src) => {
  for (const k of Object.keys(src)) {
    if (k !== 'default' && !Object.prototype.hasOwnProperty.call(ns, k)) {
      Object.defineProperty(ns, k, { get: () => src[k], enumerable: true });
    }
  }
});
hidden(G, '__ferroImport', (spec, ref) => new Promise((res, rej) => N.importModule(String(spec), String(ref), res, rej)));
