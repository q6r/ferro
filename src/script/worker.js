// Prelúdio do worker (depois do comum.js): o global DedicatedWorkerGlobalScope
// (self, location, navigator, postMessage, close, importScripts). Sem DOM.
const [NAME, MODULE] = N.workerInfo();
function baseURL() { return N.href(); }
function eventPath(t) { return [t]; }
// `function onmessage(e) {}` no topo do script também vale
function inlineHandlerOf(t, type) {
  if (t !== G) return null;
  const own = Object.getOwnPropertyDescriptor(G, 'on' + type);
  return own && typeof own.value === 'function' ? own.value : null;
}
function formEntriesOf() {
  throw new TypeError("Failed to construct 'FormData': parameter 1 is not of type 'HTMLFormElement'.");
}

// ------------------------------------------------------------- global
hidden(G, 'self', G);
hidden(G, 'name', NAME);
defineHandlers(DedicatedWorkerGlobalScope.prototype, ['message', 'messageerror']);
defineHandlers(WorkerGlobalScope.prototype, ['error', 'languagechange', 'offline', 'online', 'rejectionhandled',
  'unhandledrejection']);
methods(G, {
  postMessage(message, transfer) {
    const ctx = "Failed to execute 'postMessage' on 'DedicatedWorkerGlobalScope': ";
    if (arguments.length === 0) throw new TypeError(`${ctx}1 argument required, but only 0 present.`);
    N.scopePost(message, transferOf(transfer, ctx), cloneOut);
  },
  close() { N.scopeClose(); },
  importScripts(...urls) { N.importScripts(urls.map(String)); },
  // o Chrome também tem nos workers dedicados (OffscreenCanvas)
  requestAnimationFrame(cb) {
    if (typeof cb !== 'function') throw new TypeError("Failed to execute 'requestAnimationFrame' on 'DedicatedWorkerGlobalScope': The callback provided as parameter 1 is not a function.");
    return setTimeout(() => cb(N.now()), 16);
  },
  cancelAnimationFrame(id) { clearTimeout(id); },
});

// ------------------------------------------------------------ location
class WorkerLocation {
  constructor() { throw new TypeError('Illegal constructor'); }
}
for (const part of ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin']) {
  accessor(WorkerLocation.prototype, part, () => N.urlParse(N.href())[part]);
}
accessor(WorkerLocation.prototype, 'href', () => N.href());
methods(WorkerLocation.prototype, { toString() { return N.href(); } });
Object.defineProperty(WorkerLocation.prototype, Symbol.toStringTag, { value: 'WorkerLocation', configurable: true });
const LOC = Object.create(WorkerLocation.prototype);
accessor(G, 'location', () => LOC);
hidden(G, 'WorkerLocation', WorkerLocation);

// ----------------------------------------------------------- navigator
class WorkerNavigator {
  constructor() { throw new TypeError('Illegal constructor'); }
}
Object.defineProperty(WorkerNavigator.prototype, Symbol.toStringTag, { value: 'WorkerNavigator', configurable: true });
const NAV = Object.setPrototypeOf({
  userAgent: N.userAgent(),
  appVersion: N.userAgent().replace(/^Mozilla\//, ''),
  appName: 'Netscape', appCodeName: 'Mozilla', product: 'Gecko', platform: 'Win32',
  language: 'pt-BR', languages: Object.freeze(['pt-BR', 'pt', 'en-US', 'en']),
  onLine: true, hardwareConcurrency: 8, deviceMemory: 8,
  permissions: { query() { return Promise.resolve({ state: 'prompt', onchange: null, addEventListener() {} }); } },
  storage: { estimate() { return Promise.resolve({ quota: 1 << 30, usage: 0 }); }, persist() { return Promise.resolve(false); }, persisted() { return Promise.resolve(false); } },
  locks: { request(name, opts, cb) { cb = cb || opts; return Promise.resolve().then(() => cb({ name })); }, query() { return Promise.resolve({ held: [], pending: [] }); } },
}, WorkerNavigator.prototype);
accessor(G, 'navigator', () => NAV);
hidden(G, 'WorkerNavigator', WorkerNavigator);

// -------------------------------------------------- FileReaderSync
function blobBytes(b, how) {
  if (!(b instanceof Blob)) throw new TypeError(`Failed to execute '${how}' on 'FileReaderSync': parameter 1 is not of type 'Blob'.`);
  return BLOB.get(b).bytes;
}
class FileReaderSync {
  readAsArrayBuffer(b) { const x = blobBytes(b, 'readAsArrayBuffer'); return x.buffer.slice(x.byteOffset, x.byteOffset + x.length); }
  readAsText(b, enc) { return N.utf8Decode(blobBytes(b, 'readAsText'), enc === undefined ? 'utf-8' : String(enc)); }
  readAsDataURL(b) { const x = blobBytes(b, 'readAsDataURL'); return `data:${b.type || 'application/octet-stream'};base64,${btoa(binaryOf(x))}`; }
  readAsBinaryString(b) { return binaryOf(blobBytes(b, 'readAsBinaryString')); }
}
hidden(G, 'FileReaderSync', FileReaderSync);

// ---------------------------------------------------- ganchos do Rust
Object.assign(hooks, {
  /// Mensagem de quem criou o worker (null em `ports` = messageerror).
  scopeMessage(data, ports) {
    fireTrusted(G, ports === null ? new MessageEvent('messageerror', { data: null })
      : new MessageEvent('message', { data, ports: Object.freeze(ports) }));
  },
  /// Exceção não tratada: `error` no global; devolve se ninguém cancelou.
  scopeError(message, filename, lineno, colno, error) {
    return fireTrusted(G, new ErrorEvent('error', { message, filename, lineno, colno, error, cancelable: true }));
  },
  /// Worker de módulo: o corpo é assíncrono; um erro nele vai ao console e
  /// vira `error` (Event simples) no Worker.
  moduleRan(p) { Promise.resolve(p).catch((e) => N.moduleFailed(e)); },
});
hidden(G, '__ferroHooks', hooks);
