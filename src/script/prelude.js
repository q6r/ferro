// Prelúdio do Ferro: a parte da plataforma web escrita em JavaScript, sobre
// as funções nativas (`__native`) e as classes do DOM feitas em Rust.
(function (N) {
'use strict';
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

// ------------------------------------------------------------- janela
for (const k of ['window', 'self', 'frames', 'parent', 'top']) hidden(G, k, G);
hidden(G, 'opener', null);
hidden(G, 'closed', false);
hidden(G, 'name', '');
hidden(G, 'length', 0);
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
const MODS = { ctrlKey: false, shiftKey: false, altKey: false, metaKey: false };
const CustomEvent = eventClass('CustomEvent', Event, { detail: null });
hidden(CustomEvent.prototype, 'initCustomEvent', function (type, b, c, detail) {
  this.initEvent(type, b, c); this.detail = detail;
});
const UIEvent = eventClass('UIEvent', Event, { view: null, detail: 0, which: 0 });
const MouseEvent = eventClass('MouseEvent', UIEvent, {
  screenX: 0, screenY: 0, clientX: 0, clientY: 0, button: 0, buttons: 0, relatedTarget: null,
  movementX: 0, movementY: 0, ...MODS,
}, (e) => {
  for (const [a, b] of [['pageX', 'clientX'], ['x', 'clientX'], ['offsetX', 'clientX'], ['layerX', 'clientX']])
    Object.defineProperty(e, a, { value: e[b] + (a === 'pageX' ? G.scrollX : 0), writable: true, enumerable: true, configurable: true });
  for (const [a, b] of [['pageY', 'clientY'], ['y', 'clientY'], ['offsetY', 'clientY'], ['layerY', 'clientY']])
    Object.defineProperty(e, a, { value: e[b] + (a === 'pageY' ? G.scrollY : 0), writable: true, enumerable: true, configurable: true });
});
hidden(MouseEvent.prototype, 'getModifierState', function (k) {
  return !!{ Control: this.ctrlKey, Shift: this.shiftKey, Alt: this.altKey, Meta: this.metaKey }[k];
});
hidden(MouseEvent.prototype, 'initMouseEvent', function (type, b, c, view, detail, sx, sy, cx, cy, ctrl, alt, shift, meta, button, related) {
  this.initEvent(type, b, c);
  Object.assign(this, { view, detail, screenX: sx, screenY: sy, clientX: cx, clientY: cy, ctrlKey: ctrl, altKey: alt, shiftKey: shift, metaKey: meta, button, relatedTarget: related });
});
eventClass('PointerEvent', MouseEvent, {
  pointerId: 1, width: 1, height: 1, pressure: 0, tangentialPressure: 0, tiltX: 0, tiltY: 0, twist: 0,
  pointerType: 'mouse', isPrimary: true,
});
eventClass('WheelEvent', MouseEvent, { deltaX: 0, deltaY: 0, deltaZ: 0, deltaMode: 0 });
eventClass('DragEvent', MouseEvent, { dataTransfer: null });
const KeyboardEvent = eventClass('KeyboardEvent', UIEvent, {
  key: '', code: '', location: 0, repeat: false, isComposing: false, charCode: 0, keyCode: 0, ...MODS,
});
hidden(KeyboardEvent.prototype, 'getModifierState', MouseEvent.prototype.getModifierState);
eventClass('FocusEvent', UIEvent, { relatedTarget: null });
eventClass('InputEvent', UIEvent, { data: null, inputType: '', isComposing: false, dataTransfer: null });
eventClass('CompositionEvent', UIEvent, { data: '' });
eventClass('ErrorEvent', Event, { message: '', filename: '', lineno: 0, colno: 0, error: null });
eventClass('PopStateEvent', Event, { state: null });
eventClass('HashChangeEvent', Event, { oldURL: '', newURL: '' });
const MessageEvent = eventClass('MessageEvent', Event, { data: null, origin: '', lastEventId: '', source: null, ports: [] });
const ProgressEvent = eventClass('ProgressEvent', Event, { lengthComputable: false, loaded: 0, total: 0 });
eventClass('PageTransitionEvent', Event, { persisted: false });
eventClass('AnimationEvent', Event, { animationName: '', elapsedTime: 0, pseudoElement: '' });
eventClass('TransitionEvent', Event, { propertyName: '', elapsedTime: 0, pseudoElement: '' });
eventClass('SubmitEvent', Event, { submitter: null });
eventClass('StorageEvent', Event, { key: null, oldValue: null, newValue: null, url: '', storageArea: null });
eventClass('PromiseRejectionEvent', Event, { promise: null, reason: undefined });
eventClass('SecurityPolicyViolationEvent', Event, { blockedURI: '', violatedDirective: '' });
eventClass('ClipboardEvent', Event, { clipboardData: null });
eventClass('BeforeUnloadEvent', Event, {});

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

/// Atributos `on*` (onclick="...") viram funções na primeira vez.
function inlineHandler(el, type) {
  if (!(el instanceof Element)) return null;
  const code = el.getAttribute('on' + type);
  if (code == null) return null;
  let cache = INLINE.get(el);
  if (!cache) INLINE.set(el, cache = new Map());
  let hit = cache.get(type);
  if (hit && hit.code === code) return hit.fn;
  let fn = null;
  try { fn = new Function('event', code); } catch (e) { N.reportError(e); }
  cache.set(type, { code, fn });
  return fn;
}
const BODY_WINDOW_EVENTS = new Set(['load', 'unload', 'beforeunload', 'hashchange', 'popstate', 'message',
  'online', 'offline', 'pagehide', 'pageshow', 'resize', 'storage', 'error', 'focus', 'blur', 'scroll']);
function handlerOf(t, type) {
  const h = HANDLERS.get(t);
  const set = h && h.get(type);
  if (typeof set === 'function') return set;
  if (set === null) return null;
  if (t === G) {
    const own = Object.getOwnPropertyDescriptor(G, 'on' + type);
    if (own && typeof own.value === 'function') return own.value;
    const body = document.body;
    if (body && BODY_WINDOW_EVENTS.has(type)) return inlineHandler(body, type);
    return null;
  }
  if (t === document.body && BODY_WINDOW_EVENTS.has(type) && type !== 'scroll') return null;
  return inlineHandler(t, type);
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
function eventPath(t, s) {
  const path = [t];
  if (t instanceof Node) {
    // eventos compostos atravessam a sombra (raiz → hospedeiro)
    const up = (n) => n.parentNode || (n instanceof ShadowRoot && s.composed ? n.host : null);
    let n = up(t);
    while (n) { path.push(n); n = up(n); }
    if (path[path.length - 1] === document && s.type !== 'load') path.push(G);
  }
  return path;
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

// propriedades onclick, onload... (o valor fica guardado à parte)
const HANDLER_NAMES = ('abort auxclick beforeinput blur cancel canplay canplaythrough change click close contextmenu ' +
  'copy cut dblclick drag dragend dragenter dragleave dragover dragstart drop durationchange ended error focus ' +
  'focusin focusout input invalid keydown keypress keyup load loadeddata loadedmetadata loadstart mousedown ' +
  'mouseenter mouseleave mousemove mouseout mouseover mouseup paste pause play playing pointercancel pointerdown ' +
  'pointerenter pointerleave pointermove pointerout pointerover pointerup progress ratechange reset resize ' +
  'scroll scrollend seeked seeking select selectionchange selectstart stalled submit suspend timeupdate toggle ' +
  'volumechange waiting wheel animationstart animationend animationiteration transitionstart transitionend ' +
  'transitionrun transitioncancel').split(' ');
const WINDOW_HANDLERS = ['afterprint', 'beforeprint', 'beforeunload', 'hashchange', 'languagechange', 'message',
  'messageerror', 'offline', 'online', 'pagehide', 'pageshow', 'popstate', 'rejectionhandled', 'storage',
  'unhandledrejection', 'unload'];
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
defineHandlers(HTMLElement.prototype, HANDLER_NAMES);
defineHandlers(SVGElement.prototype, HANDLER_NAMES);
defineHandlers(Document.prototype, HANDLER_NAMES.concat(['readystatechange', 'visibilitychange']));
defineHandlers(Window.prototype, HANDLER_NAMES.concat(WINDOW_HANDLERS));

// ------------------------------------------------------- coleções do DOM
const AP = Array.prototype;
for (const P of [NodeList.prototype, HTMLCollection.prototype, NamedNodeMap.prototype]) {
  hidden(P, 'item', function item(i) { const v = this[Number(i) >>> 0]; return v === undefined ? null : v; });
  hidden(P, Symbol.iterator, AP.values);
}
methods(NodeList.prototype, { forEach: AP.forEach, entries: AP.entries, keys: AP.keys, values: AP.values });
methods(HTMLCollection.prototype, {
  namedItem(name) {
    for (const e of this) if (e.id === name || e.getAttribute('name') === name) return e;
    return null;
  },
});
methods(NamedNodeMap.prototype, {
  getNamedItem(name) {
    name = String(name).toLowerCase();
    for (const a of this) if (a.name === name) return a;
    return null;
  },
});

// ------------------------------------------------------------ DOMRect
class DOMRectReadOnly {
  constructor(x = 0, y = 0, width = 0, height = 0) {
    Object.assign(this, { x: +x, y: +y, width: +width, height: +height });
  }
  get top() { return Math.min(this.y, this.y + this.height); }
  get left() { return Math.min(this.x, this.x + this.width); }
  get right() { return Math.max(this.x, this.x + this.width); }
  get bottom() { return Math.max(this.y, this.y + this.height); }
  toJSON() {
    const { x, y, width, height, top, left, right, bottom } = this;
    return { x, y, width, height, top, left, right, bottom };
  }
  static fromRect(r = {}) { return new this(r.x, r.y, r.width, r.height); }
}
class DOMRect extends DOMRectReadOnly {}
hidden(G, 'DOMRectReadOnly', DOMRectReadOnly);
hidden(G, 'DOMRect', DOMRect);

// ----------------------------------------------------------- classList
const TOKENS = new WeakMap();
class DOMTokenList {
  constructor() { throw new TypeError('Illegal constructor'); }
}
function tokensOf(list) {
  const { el, attr } = TOKENS.get(list);
  const v = el.getAttribute(attr) || '';
  return [...new Set(v.split(/[ \t\n\f\r]+/).filter(Boolean))];
}
function writeTokens(list, toks) {
  const { el, attr } = TOKENS.get(list);
  el.setAttribute(attr, toks.join(' '));
}
function checkToken(t, fn) {
  t = String(t);
  if (t === '') throw new DOMException(`Failed to execute '${fn}' on 'DOMTokenList': The token provided must not be empty.`, 'SyntaxError');
  if (/\s/.test(t)) throw new DOMException(`Failed to execute '${fn}' on 'DOMTokenList': The token provided ('${t}') contains HTML space characters, which are not valid in tokens.`, 'InvalidCharacterError');
  return t;
}
methods(DOMTokenList.prototype, {
  item(i) { return tokensOf(this)[i] ?? null; },
  contains(t) { return tokensOf(this).includes(String(t)); },
  add(...ts) {
    const toks = tokensOf(this);
    for (let t of ts) { t = checkToken(t, 'add'); if (!toks.includes(t)) toks.push(t); }
    writeTokens(this, toks);
  },
  remove(...ts) {
    ts = ts.map((t) => checkToken(t, 'remove'));
    const toks = tokensOf(this).filter((t) => !ts.includes(t));
    const { el, attr } = TOKENS.get(this);
    if (el.hasAttribute(attr)) writeTokens(this, toks);
  },
  toggle(t, force) {
    t = checkToken(t, 'toggle');
    const has = this.contains(t);
    const want = force === undefined ? !has : !!force;
    if (want && !has) this.add(t);
    else if (!want && has) this.remove(t);
    return want;
  },
  replace(a, b) {
    a = checkToken(a, 'replace'); b = checkToken(b, 'replace');
    const toks = tokensOf(this);
    const i = toks.indexOf(a);
    if (i < 0) return false;
    toks[i] = b;
    writeTokens(this, [...new Set(toks)]);
    return true;
  },
  supports() { return true; },
  toString() { return this.value; },
  forEach(cb, thisArg) { tokensOf(this).forEach((t, i) => cb.call(thisArg, t, i, this)); },
  entries() { return tokensOf(this).entries(); },
  keys() { return tokensOf(this).keys(); },
  values() { return tokensOf(this).values(); },
  [Symbol.iterator]() { return tokensOf(this).values(); },
});
accessor(DOMTokenList.prototype, 'length', function () { return tokensOf(this).length; });
accessor(DOMTokenList.prototype, 'value', function () {
  const { el, attr } = TOKENS.get(this);
  return el.getAttribute(attr) || '';
}, function (v) {
  const { el, attr } = TOKENS.get(this);
  el.setAttribute(attr, String(v));
});
hidden(G, 'DOMTokenList', DOMTokenList);
const TOKEN_CACHE = new WeakMap();
function tokenList(el, attr) {
  let m = TOKEN_CACHE.get(el);
  if (!m) TOKEN_CACHE.set(el, m = {});
  if (!m[attr]) {
    const l = Object.create(DOMTokenList.prototype);
    TOKENS.set(l, { el, attr });
    m[attr] = l;
  }
  return m[attr];
}
accessor(Element.prototype, 'classList', function () { return tokenList(this, 'class'); },
  function (v) { this.setAttribute('class', String(v)); });
for (const P of [HTMLAnchorElement.prototype, HTMLLinkElement.prototype, HTMLAreaElement.prototype])
  accessor(P, 'relList', function () { return tokenList(this, 'rel'); });

// --------------------------------------------------------------- style
const STYLE = new WeakMap();
function splitDecls(text) {
  const out = [];
  let depth = 0, quote = '', cur = '';
  for (const c of text) {
    if (quote) { if (c === quote) quote = ''; cur += c; continue; }
    if (c === '"' || c === "'") { quote = c; cur += c; continue; }
    if (c === '(') depth++;
    else if (c === ')') depth = Math.max(0, depth - 1);
    if (c === ';' && depth === 0) { out.push(cur); cur = ''; continue; }
    cur += c;
  }
  out.push(cur);
  return out;
}
function parseStyle(text) {
  const m = new Map();
  for (const d of splitDecls(text || '')) {
    const i = d.indexOf(':');
    if (i < 0) continue;
    let name = d.slice(0, i).trim();
    if (!name) continue;
    if (!name.startsWith('--')) name = name.toLowerCase();
    let value = d.slice(i + 1).trim();
    let important = '';
    const imp = /!\s*important\s*$/i.exec(value);
    if (imp) { value = value.slice(0, imp.index).trim(); important = 'important'; }
    if (value === '') continue;
    m.delete(name);
    m.set(name, { value, important });
  }
  return m;
}
function serializeStyle(m) {
  return [...m].map(([k, v]) => `${k}: ${v.value}${v.important ? ' !important' : ''};`).join(' ');
}
class CSSStyleDeclaration {
  constructor() { throw new TypeError('Illegal constructor'); }
}
function styleMap(decl) {
  const s = STYLE.get(decl);
  if (s.computed) return s.computed;
  return parseStyle(s.el.getAttribute('style'));
}
function writeStyle(decl, m) {
  const s = STYLE.get(decl);
  if (s.computed) throw new DOMException("Failed to execute 'setProperty' on 'CSSStyleDeclaration': These styles are computed, and therefore read-only.", 'NoModificationAllowedError');
  s.el.setAttribute('style', serializeStyle(m));
}
const UNITLESS = new Set(['opacity', 'z-index', 'flex', 'flex-grow', 'flex-shrink', 'order', 'line-height',
  'font-weight', 'zoom', 'orphans', 'widows', 'column-count', 'fill-opacity', 'stroke-opacity', 'tab-size',
  'animation-iteration-count', 'grid-row', 'grid-column', 'stroke-width', 'aspect-ratio']);
methods(CSSStyleDeclaration.prototype, {
  getPropertyValue(name) {
    name = String(name);
    if (!name.startsWith('--')) name = name.toLowerCase();
    const v = styleMap(this).get(name);
    return v ? v.value : '';
  },
  getPropertyPriority(name) {
    const v = styleMap(this).get(String(name).toLowerCase());
    return v ? v.important : '';
  },
  setProperty(name, value, priority) {
    name = String(name);
    if (!name.startsWith('--')) name = name.toLowerCase();
    if (value === null || value === undefined || String(value) === '') { this.removeProperty(name); return; }
    value = String(value);
    // número sem unidade só vale para algumas propriedades (e para 0)
    if (/^-?\d+(\.\d+)?$/.test(value) && value !== '0' && !UNITLESS.has(name) && !name.startsWith('--')) return;
    const m = styleMap(this);
    m.delete(name);
    m.set(name, { value, important: priority === 'important' ? 'important' : '' });
    writeStyle(this, m);
  },
  removeProperty(name) {
    name = String(name);
    if (!name.startsWith('--')) name = name.toLowerCase();
    const m = styleMap(this);
    const old = m.get(name);
    if (!old) return '';
    m.delete(name);
    writeStyle(this, m);
    return old.value;
  },
  item(i) { return [...styleMap(this).keys()][i] ?? ''; },
  [Symbol.iterator]() { return styleMap(this).keys(); },
});
accessor(CSSStyleDeclaration.prototype, 'length', function () { return styleMap(this).size; });
accessor(CSSStyleDeclaration.prototype, 'cssText', function () {
  return STYLE.get(this).computed ? '' : serializeStyle(styleMap(this));
}, function (v) {
  const s = STYLE.get(this);
  if (!s.computed) s.el.setAttribute('style', serializeStyle(parseStyle(String(v))));
});
accessor(CSSStyleDeclaration.prototype, 'parentRule', () => null);
const CSS_PROPS = ('align-content align-items align-self all animation animation-delay animation-direction ' +
  'animation-duration animation-fill-mode animation-iteration-count animation-name animation-play-state ' +
  'animation-timing-function appearance aspect-ratio backdrop-filter backface-visibility background ' +
  'background-attachment background-blend-mode background-clip background-color background-image ' +
  'background-origin background-position background-position-x background-position-y background-repeat ' +
  'background-size block-size border border-block border-bottom border-bottom-color border-bottom-left-radius ' +
  'border-bottom-right-radius border-bottom-style border-bottom-width border-collapse border-color border-image ' +
  'border-inline border-left border-left-color border-left-style border-left-width border-radius border-right ' +
  'border-right-color border-right-style border-right-width border-spacing border-style border-top ' +
  'border-top-color border-top-left-radius border-top-right-radius border-top-style border-top-width ' +
  'border-width bottom box-shadow box-sizing break-after break-before break-inside caption-side caret-color ' +
  'clear clip clip-path color column-count column-gap column-rule column-span column-width columns contain ' +
  'container content content-visibility counter-increment counter-reset cursor direction display empty-cells ' +
  'fill fill-opacity filter flex flex-basis flex-direction flex-flow flex-grow flex-shrink flex-wrap float ' +
  'font font-family font-feature-settings font-kerning font-size font-stretch font-style font-variant ' +
  'font-variant-numeric font-weight gap grid grid-area grid-auto-columns grid-auto-flow grid-auto-rows ' +
  'grid-column grid-column-end grid-column-start grid-row grid-row-end grid-row-start grid-template ' +
  'grid-template-areas grid-template-columns grid-template-rows height hyphens image-rendering inline-size ' +
  'inset inset-block inset-inline isolation justify-content justify-items justify-self left letter-spacing ' +
  'line-break line-clamp line-height list-style list-style-image list-style-position list-style-type margin ' +
  'margin-block margin-block-end margin-block-start margin-bottom margin-inline margin-inline-end ' +
  'margin-inline-start margin-left margin-right margin-top mask mask-image max-block-size max-height ' +
  'max-inline-size max-width min-block-size min-height min-inline-size min-width mix-blend-mode object-fit ' +
  'object-position opacity order outline outline-color outline-offset outline-style outline-width overflow ' +
  'overflow-anchor overflow-wrap overflow-x overflow-y overscroll-behavior padding padding-block ' +
  'padding-block-end padding-block-start padding-bottom padding-inline padding-inline-end padding-inline-start ' +
  'padding-left padding-right padding-top page-break-after page-break-before page-break-inside perspective ' +
  'perspective-origin place-content place-items place-self pointer-events position quotes resize right rotate ' +
  'row-gap scale scroll-behavior scroll-margin scroll-padding scroll-snap-align scroll-snap-type scrollbar-color ' +
  'scrollbar-gutter scrollbar-width shape-outside stroke stroke-dasharray stroke-dashoffset stroke-linecap ' +
  'stroke-linejoin stroke-opacity stroke-width tab-size table-layout text-align text-align-last ' +
  'text-decoration text-decoration-color text-decoration-line text-decoration-style text-decoration-thickness ' +
  'text-indent text-overflow text-rendering text-shadow text-transform text-underline-offset ' +
  'text-underline-position top touch-action transform transform-origin transform-style transition ' +
  'transition-delay transition-duration transition-property transition-timing-function translate ' +
  'unicode-bidi user-select vertical-align visibility white-space width will-change word-break ' +
  'word-spacing word-wrap writing-mode z-index zoom -webkit-appearance -webkit-line-clamp ' +
  '-webkit-box-orient -webkit-font-smoothing -webkit-text-size-adjust -webkit-user-select ' +
  '-webkit-tap-highlight-color -webkit-transform -webkit-transition -webkit-mask-image -webkit-overflow-scrolling ' +
  '-webkit-backdrop-filter -moz-osx-font-smoothing').split(' ');
const camel = (p) => (p.startsWith('-webkit-') ? 'webkit' + p.slice(7) : p.startsWith('-moz-') ? 'moz' + p.slice(4) : p)
  .replace(/-([a-z])/g, (_, c) => c.toUpperCase());
for (const p of CSS_PROPS) {
  const get = function () { return this.getPropertyValue(p); };
  const set = function (v) { this.setProperty(p, v); };
  accessor(CSSStyleDeclaration.prototype, camel(p), get, set);
  if (p.includes('-')) accessor(CSSStyleDeclaration.prototype, p, get, set);
  if (p.startsWith('-webkit-')) accessor(CSSStyleDeclaration.prototype, 'Webkit' + camel(p).slice(6), get, set);
}
accessor(CSSStyleDeclaration.prototype, 'cssFloat', function () { return this.getPropertyValue('float'); },
  function (v) { this.setProperty('float', v); });
hidden(G, 'CSSStyleDeclaration', CSSStyleDeclaration);
const STYLE_CACHE = new WeakMap();
for (const P of [HTMLElement.prototype, SVGElement.prototype]) {
  accessor(P, 'style', function () {
    let d = STYLE_CACHE.get(this);
    if (!d) {
      d = Object.create(CSSStyleDeclaration.prototype);
      STYLE.set(d, { el: this });
      STYLE_CACHE.set(this, d);
    }
    return d;
  }, function (v) { this.setAttribute('style', String(v)); });
}
hidden(G, 'getComputedStyle', function getComputedStyle(el) {
  if (!(el instanceof Element)) throw new TypeError("Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'.");
  const raw = N.computedStyle(el) || {};
  const m = new Map();
  for (const k of Object.keys(raw)) m.set(k, { value: raw[k], important: '' });
  const d = Object.create(CSSStyleDeclaration.prototype);
  STYLE.set(d, { el, computed: m });
  return d;
});

// -------------------------------------------------------------- dataset
class DOMStringMap {
  constructor() { throw new TypeError('Illegal constructor'); }
}
hidden(G, 'DOMStringMap', DOMStringMap);
const DATASET = new WeakMap();
const toDataAttr = (k) => 'data-' + k.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
const toDataProp = (a) => a.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
for (const P of [HTMLElement.prototype, SVGElement.prototype]) {
  accessor(P, 'dataset', function () {
    let d = DATASET.get(this);
    if (d) return d;
    const el = this;
    d = new Proxy(Object.create(DOMStringMap.prototype), {
      get(t, k) {
        if (typeof k !== 'string') return t[k];
        const v = el.getAttribute(toDataAttr(k));
        return v === null ? undefined : v;
      },
      set(t, k, v) {
        if (typeof k !== 'string') { t[k] = v; return true; }
        el.setAttribute(toDataAttr(k), String(v));
        return true;
      },
      has(t, k) { return typeof k === 'string' && el.hasAttribute(toDataAttr(k)); },
      deleteProperty(t, k) { el.removeAttribute(toDataAttr(String(k))); return true; },
      ownKeys() { return el.getAttributeNames().filter((n) => n.startsWith('data-')).map(toDataProp); },
      getOwnPropertyDescriptor(t, k) {
        if (typeof k !== 'string') return undefined;
        const v = el.getAttribute(toDataAttr(k));
        return v === null ? undefined : { value: v, writable: true, enumerable: true, configurable: true };
      },
    });
    DATASET.set(this, d);
    return d;
  });
}

// ------------------------------------------------ atributos refletidos
function resolveURL(v) {
  if (v === null) return '';
  const p = N.urlParse(v, document.baseURI);
  return p ? p.href : v;
}
function reflect(P, prop, attr, kind, def) {
  attr = attr || prop.toLowerCase();
  let get, set;
  switch (kind) {
    case 'bool':
      get = function () { return this.hasAttribute(attr); };
      set = function (v) { if (v) this.setAttribute(attr, ''); else this.removeAttribute(attr); };
      break;
    case 'url':
      get = function () { const v = this.getAttribute(attr); return v === null ? '' : resolveURL(v.trim()); };
      set = function (v) { this.setAttribute(attr, String(v)); };
      break;
    case 'int':
      get = function () {
        const v = parseInt(this.getAttribute(attr), 10);
        return Number.isFinite(v) ? v : (typeof def === 'function' ? def.call(this) : def ?? 0);
      };
      set = function (v) { this.setAttribute(attr, String(Math.trunc(Number(v)) || 0)); };
      break;
    case 'lower':
      get = function () { const v = this.getAttribute(attr); return v === null ? (def ?? '') : v.toLowerCase(); };
      set = function (v) { this.setAttribute(attr, String(v)); };
      break;
    default:
      get = function () { const v = this.getAttribute(attr); return v === null ? (def ?? '') : v; };
      set = function (v) { this.setAttribute(attr, v === null ? '' : String(v)); };
  }
  accessor(P, prop, get, set);
}
function reflectAll(P, list) {
  for (const spec of list) {
    if (typeof spec === 'string') reflect(P, spec);
    else reflect(P, ...spec);
  }
}
const HE = HTMLElement.prototype;
const FOCUSABLE = new Set(['a', 'button', 'input', 'select', 'textarea', 'iframe', 'summary']);
reflectAll(HE, ['title', 'lang', 'dir', 'accessKey', 'nonce', 'slot', ['hidden', 'hidden', 'bool'],
  ['autofocus', 'autofocus', 'bool'], ['inert', 'inert', 'bool'], 'enterKeyHint', 'inputMode', 'popover',
  ['tabIndex', 'tabindex', 'int', function () { return FOCUSABLE.has(this.localName) || this.isContentEditable ? 0 : -1; }]]);
reflectAll(SVGElement.prototype, [['tabIndex', 'tabindex', 'int', -1]]);
accessor(HE, 'draggable', function () { return this.getAttribute('draggable') === 'true' || this.localName === 'img'; },
  function (v) { this.setAttribute('draggable', v ? 'true' : 'false'); });
accessor(HE, 'spellcheck', function () { return this.getAttribute('spellcheck') !== 'false'; },
  function (v) { this.setAttribute('spellcheck', v ? 'true' : 'false'); });
accessor(HE, 'translate', function () { return this.getAttribute('translate') !== 'no'; },
  function (v) { this.setAttribute('translate', v ? 'yes' : 'no'); });
accessor(HE, 'contentEditable', function () {
  const v = this.getAttribute('contenteditable');
  return v === null ? 'inherit' : v === '' ? 'true' : v.toLowerCase();
}, function (v) { this.setAttribute('contenteditable', String(v)); });
accessor(HE, 'isContentEditable', function () {
  for (let e = this; e; e = e.parentElement) {
    const v = e.getAttribute('contenteditable');
    if (v !== null) return v !== 'false';
  }
  return false;
});

// URL decomposta (a, area): hostname, pathname...
function urlParts(P) {
  for (const part of ['protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash']) {
    accessor(P, part, function () {
      const p = N.urlParse(this.href);
      return p ? p[part] : '';
    }, function (v) {
      const href = this.href;
      if (href) this.setAttribute('href', N.urlSet(href, part, String(v)));
    });
  }
  accessor(P, 'origin', function () { const p = N.urlParse(this.href); return p ? p.origin : ''; });
  hidden(P, 'toString', function () { return this.href; });
}
reflectAll(HTMLAnchorElement.prototype, [['href', 'href', 'url'], 'target', 'rel', 'download', 'hreflang', 'type',
  ['referrerPolicy', 'referrerpolicy'], 'ping', 'name']);
accessor(HTMLAnchorElement.prototype, 'text', function () { return this.textContent; }, function (v) { this.textContent = v; });
urlParts(HTMLAnchorElement.prototype);
reflectAll(HTMLAreaElement.prototype, [['href', 'href', 'url'], 'target', 'rel', 'download', 'alt', 'coords', 'shape']);
urlParts(HTMLAreaElement.prototype);

const IMG = HTMLImageElement.prototype;
reflectAll(IMG, [['src', 'src', 'url'], 'alt', 'srcset', 'sizes', ['crossOrigin', 'crossorigin'], 'loading',
  'decoding', ['useMap', 'usemap'], ['isMap', 'ismap', 'bool'], ['referrerPolicy', 'referrerpolicy'],
  ['fetchPriority', 'fetchpriority'], 'name', 'align', 'border']);
reflect(IMG, 'width', 'width', 'int', 0);
reflect(IMG, 'height', 'height', 'int', 0);
accessor(IMG, 'naturalWidth', function () { return this.width; });
accessor(IMG, 'naturalHeight', function () { return this.height; });
accessor(IMG, 'complete', () => true);
accessor(IMG, 'currentSrc', function () { return this.src; });
hidden(IMG, 'decode', function () { return Promise.resolve(); });

// formulários
const VALUES = new WeakMap();
function formOf(el) {
  const id = el.getAttribute('form');
  if (id) return document.getElementById(id);
  return el.closest('form');
}
const INPUT = HTMLInputElement.prototype;
reflectAll(INPUT, ['name', 'placeholder', 'min', 'max', 'step', 'pattern', 'autocomplete', 'accept', 'alt',
  ['src', 'src', 'url'], ['formAction', 'formaction', 'url'], ['disabled', 'disabled', 'bool'],
  ['readOnly', 'readonly', 'bool'], ['required', 'required', 'bool'], ['multiple', 'multiple', 'bool'],
  ['defaultChecked', 'checked', 'bool'], ['defaultValue', 'value'], ['maxLength', 'maxlength', 'int', -1],
  ['minLength', 'minlength', 'int', -1], ['size', 'size', 'int', 20], 'dirName', ['inputMode', 'inputmode']]);
const INPUT_TYPES = new Set(['hidden', 'text', 'search', 'tel', 'url', 'email', 'password', 'date', 'month', 'week',
  'time', 'datetime-local', 'number', 'range', 'color', 'checkbox', 'radio', 'file', 'submit', 'image', 'reset', 'button']);
accessor(INPUT, 'type', function () {
  const t = (this.getAttribute('type') || '').toLowerCase();
  return INPUT_TYPES.has(t) ? t : 'text';
}, function (v) { this.setAttribute('type', String(v)); });
accessor(INPUT, 'value', function () {
  const v = VALUES.get(this);
  if (v !== undefined) return v;
  const a = this.getAttribute('value');
  if (a === null) return (this.type === 'checkbox' || this.type === 'radio') ? 'on' : '';
  return a;
}, function (v) {
  v = v === null ? '' : String(v);
  VALUES.set(this, v);
  // o desenho do campo usa o atributo
  if (this.type !== 'checkbox' && this.type !== 'radio' && this.type !== 'file') this.setAttribute('value', v);
});
accessor(INPUT, 'checked', function () { return this.hasAttribute('checked'); }, function (v) {
  if (v) {
    if (this.type === 'radio' && this.name) {
      const scope = formOf(this) || document;
      for (const o of scope.querySelectorAll(`input[type=radio][name="${CSS.escape(this.name)}"]`))
        if (o !== this) o.removeAttribute('checked');
    }
    this.setAttribute('checked', '');
  } else this.removeAttribute('checked');
});
accessor(INPUT, 'valueAsNumber', function () { const n = parseFloat(this.value); return Number.isFinite(n) ? n : NaN; },
  function (v) { this.value = String(v); });
accessor(INPUT, 'indeterminate', function () { return false; }, function () {});
accessor(INPUT, 'files', function () { return this.type === 'file' ? [] : null; });
accessor(INPUT, 'list', function () { const id = this.getAttribute('list'); return id ? document.getElementById(id) : null; });

