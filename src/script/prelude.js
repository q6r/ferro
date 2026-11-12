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

/// O que todos os controles de formulário têm em comum.
function formControl(P, extra) {
  accessor(P, 'form', function () { return formOf(this); });
  accessor(P, 'labels', function () {
    const out = [];
    if (this.id) for (const l of document.querySelectorAll('label')) if (l.htmlFor === this.id) out.push(l);
    const p = this.closest('label');
    if (p && !out.includes(p)) out.push(p);
    return out;
  });
  accessor(P, 'validity', () => ({ valid: true, valueMissing: false, typeMismatch: false, patternMismatch: false,
    tooLong: false, tooShort: false, rangeUnderflow: false, rangeOverflow: false, stepMismatch: false,
    badInput: false, customError: false }));
  accessor(P, 'validationMessage', () => '');
  accessor(P, 'willValidate', () => true);
  methods(P, {
    checkValidity() { return true; },
    reportValidity() { return true; },
    setCustomValidity() {},
    ...extra,
  });
}
const SEL = new WeakMap();
formControl(INPUT, {
  select() {},
  setSelectionRange(a, b) { SEL.set(this, [a, b]); },
  setRangeText() {},
  stepUp() {}, stepDown() {},
  showPicker() {},
});
accessor(INPUT, 'selectionStart', function () { return (SEL.get(this) || [this.value.length])[0]; }, function (v) { SEL.set(this, [v, v]); });
accessor(INPUT, 'selectionEnd', function () { const s = SEL.get(this); return s ? s[1] : this.value.length; }, function () {});
accessor(INPUT, 'selectionDirection', () => 'none', () => {});

const TA = HTMLTextAreaElement.prototype;
reflectAll(TA, ['name', 'placeholder', 'autocomplete', 'wrap', ['disabled', 'disabled', 'bool'],
  ['readOnly', 'readonly', 'bool'], ['required', 'required', 'bool'], ['maxLength', 'maxlength', 'int', -1],
  ['minLength', 'minlength', 'int', -1], ['rows', 'rows', 'int', 2], ['cols', 'cols', 'int', 20]]);
accessor(TA, 'type', () => 'textarea');
accessor(TA, 'value', function () { const v = VALUES.get(this); return v !== undefined ? v : this.textContent; },
  function (v) { v = v === null ? '' : String(v); VALUES.set(this, v); this.textContent = v; });
accessor(TA, 'defaultValue', function () { return this.textContent; }, function (v) { this.textContent = v; });
accessor(TA, 'textLength', function () { return this.value.length; });
formControl(TA, { select() {}, setSelectionRange(a, b) { SEL.set(this, [a, b]); }, setRangeText() {} });
accessor(TA, 'selectionStart', function () { return (SEL.get(this) || [this.value.length])[0]; }, function (v) { SEL.set(this, [v, v]); });
accessor(TA, 'selectionEnd', function () { const s = SEL.get(this); return s ? s[1] : this.value.length; }, function () {});

const OPT = HTMLOptionElement.prototype;
reflectAll(OPT, [['disabled', 'disabled', 'bool'], ['defaultSelected', 'selected', 'bool']]);
accessor(OPT, 'value', function () { const v = this.getAttribute('value'); return v === null ? this.text : v; },
  function (v) { this.setAttribute('value', String(v)); });
accessor(OPT, 'text', function () { return this.textContent.replace(/\s+/g, ' ').trim(); }, function (v) { this.textContent = v; });
accessor(OPT, 'label', function () { const v = this.getAttribute('label'); return v === null ? this.text : v; },
  function (v) { this.setAttribute('label', String(v)); });
accessor(OPT, 'selected', function () {
  if (this.hasAttribute('selected')) return true;
  const sel = this.closest('select');
  if (!sel || sel.multiple) return false;
  const opts = [...sel.options];
  return !opts.some((o) => o.hasAttribute('selected')) && opts[0] === this;
}, function (v) {
  const sel = this.closest('select');
  if (v && sel && !sel.multiple) for (const o of sel.options) o.removeAttribute('selected');
  if (v) this.setAttribute('selected', ''); else this.removeAttribute('selected');
});
accessor(OPT, 'index', function () { const sel = this.closest('select'); return sel ? [...sel.options].indexOf(this) : 0; });
accessor(OPT, 'form', function () { const sel = this.closest('select'); return sel ? sel.form : null; });

const SELECT = HTMLSelectElement.prototype;
reflectAll(SELECT, ['name', 'autocomplete', ['disabled', 'disabled', 'bool'], ['multiple', 'multiple', 'bool'],
  ['required', 'required', 'bool'], ['size', 'size', 'int', 0]]);
accessor(SELECT, 'type', function () { return this.multiple ? 'select-multiple' : 'select-one'; });
accessor(SELECT, 'options', function () {
  const list = [...this.querySelectorAll('option')];
  Object.setPrototypeOf(list, HTMLCollection.prototype);
  return list;
});
accessor(SELECT, 'length', function () { return this.options.length; });
accessor(SELECT, 'selectedOptions', function () { return [...this.options].filter((o) => o.selected); });
accessor(SELECT, 'selectedIndex', function () {
  return [...this.options].findIndex((o) => o.selected);
}, function (i) {
  [...this.options].forEach((o, j) => { if (j === Number(i)) o.selected = true; else o.removeAttribute('selected'); });
});
accessor(SELECT, 'value', function () {
  const o = [...this.options].find((o) => o.selected);
  return o ? o.value : '';
}, function (v) {
  v = String(v);
  for (const o of this.options) { if (o.value === v) o.selected = true; else o.removeAttribute('selected'); }
});
formControl(SELECT, {
  item(i) { return this.options[i] || null; },
  namedItem(n) { return [...this.options].find((o) => o.id === n || o.getAttribute('name') === n) || null; },
  add(opt, before) {
    const ref = typeof before === 'number' ? this.options[before] : before;
    if (ref) ref.before(opt); else this.appendChild(opt);
  },
  remove(i) {
    if (arguments.length === 0) { Element.prototype.remove.call(this); return; }
    const o = this.options[i];
    if (o) o.remove();
  },
  showPicker() {},
});

const BUTTON = HTMLButtonElement.prototype;
reflectAll(BUTTON, ['name', 'value', ['disabled', 'disabled', 'bool'], ['formAction', 'formaction', 'url'],
  ['formMethod', 'formmethod'], ['formNoValidate', 'formnovalidate', 'bool'], ['formTarget', 'formtarget'],
  'popoverTargetElement']);
accessor(BUTTON, 'type', function () {
  const t = (this.getAttribute('type') || '').toLowerCase();
  return t === 'button' || t === 'reset' ? t : 'submit';
}, function (v) { this.setAttribute('type', String(v)); });
formControl(BUTTON, {});
formControl(HTMLOutputElement.prototype, {});
formControl(HTMLFieldSetElement.prototype, {});
reflectAll(HTMLFieldSetElement.prototype, ['name', ['disabled', 'disabled', 'bool']]);

const FORM = HTMLFormElement.prototype;
reflectAll(FORM, ['name', 'target', 'enctype', 'accept-charset', 'autocomplete', ['noValidate', 'novalidate', 'bool'],
  ['acceptCharset', 'accept-charset'], 'rel']);
accessor(FORM, 'action', function () {
  const a = this.getAttribute('action');
  return a === null || a === '' ? document.URL : resolveURL(a);
}, function (v) { this.setAttribute('action', String(v)); });
accessor(FORM, 'method', function () {
  return (this.getAttribute('method') || '').toLowerCase() === 'post' ? 'post' : 'get';
}, function (v) { this.setAttribute('method', String(v)); });
accessor(FORM, 'encoding', function () { return this.enctype; }, function (v) { this.enctype = v; });
const CONTROLS = 'input, select, textarea, button, fieldset, output, object';
accessor(FORM, 'elements', function () {
  const id = this.id;
  const list = [...this.querySelectorAll(CONTROLS)];
  if (id) for (const e of document.querySelectorAll(`[form="${CSS.escape(id)}"]`)) if (!list.includes(e)) list.push(e);
  Object.setPrototypeOf(list, HTMLCollection.prototype);
  return list;
});
accessor(FORM, 'length', function () { return this.elements.length; });
/// Pares nome/valor que um formulário envia.
function formEntries(form, submitter) {
  const out = [];
  for (const el of form.elements) {
    const name = el.getAttribute('name');
    if (!name || el.disabled || el.closest('fieldset[disabled]')) continue;
    const tag = el.localName;
    if (tag === 'input') {
      const t = el.type;
      if ((t === 'checkbox' || t === 'radio') && !el.checked) continue;
      if (t === 'submit' || t === 'image' || t === 'button' || t === 'reset') { if (el !== submitter) continue; }
      if (t === 'file') continue;
      out.push([name, el.value]);
    } else if (tag === 'select') {
      for (const o of el.options) if (o.selected) out.push([name, o.value]);
    } else if (tag === 'textarea') {
      out.push([name, el.value]);
    } else if (tag === 'button') {
      if (el === submitter) out.push([name, el.value]);
    }
  }
  return out;
}
function submitForm(form, submitter) {
  const method = ((submitter && submitter.getAttribute('formmethod')) || form.method).toLowerCase();
  const action = (submitter && submitter.getAttribute('formaction')) ? resolveURL(submitter.getAttribute('formaction')) : form.action;
  const entries = formEntries(form, submitter);
  const query = entries.map(([k, v]) => N.formEncode(k) + '=' + N.formEncode(v)).join('&');
  if (method === 'get') {
    N.navigate(N.urlSet(action, 'search', query));
  } else if ((form.getAttribute('enctype') || '').toLowerCase() === 'multipart/form-data') {
    const fd = new FormData();
    for (const [k, v] of entries) fd.append(k, v);
    const [bytes, ct] = encodeBody(fd);
    N.navigatePost(action, ct, bytes);
  } else {
    N.navigatePost(action, 'application/x-www-form-urlencoded', query);
  }
}
methods(FORM, {
  submit() { submitForm(this, null); },
  requestSubmit(submitter) {
    const ev = new (G.SubmitEvent)('submit', { bubbles: true, cancelable: true, submitter: submitter || null });
    if (this.dispatchEvent(ev)) submitForm(this, submitter || null);
  },
  reset() {
    for (const el of this.elements) VALUES.delete(el);
    this.dispatchEvent(new Event('reset', { bubbles: true, cancelable: true }));
  },
  checkValidity() { return true; },
  reportValidity() { return true; },
});
reflectAll(HTMLLabelElement.prototype, [['htmlFor', 'for']]);
accessor(HTMLLabelElement.prototype, 'control', function () {
  const f = this.htmlFor;
  return f ? document.getElementById(f) : this.querySelector('input, select, textarea, button');
});
accessor(HTMLLabelElement.prototype, 'form', function () { const c = this.control; return c ? c.form : null; });

// script, link, style, meta, base, iframe...
reflectAll(HTMLScriptElement.prototype, [['src', 'src', 'url'], 'type', 'charset', ['async', 'async', 'bool'],
  ['defer', 'defer', 'bool'], ['crossOrigin', 'crossorigin'], ['noModule', 'nomodule', 'bool'], 'integrity',
  ['referrerPolicy', 'referrerpolicy'], 'event', ['htmlFor', 'for'], ['fetchPriority', 'fetchpriority']]);
accessor(HTMLScriptElement.prototype, 'text', function () { return this.textContent; }, function (v) { this.textContent = v; });
reflectAll(HTMLLinkElement.prototype, [['href', 'href', 'url'], 'rel', 'type', 'media', 'as', 'sizes', 'hreflang',
  ['crossOrigin', 'crossorigin'], 'integrity', ['referrerPolicy', 'referrerpolicy'], ['disabled', 'disabled', 'bool'],
  ['imageSrcset', 'imagesrcset'], ['imageSizes', 'imagesizes'], ['fetchPriority', 'fetchpriority']]);
accessor(HTMLLinkElement.prototype, 'sheet', function () {
  return /\bstylesheet\b/i.test(this.rel) ? sheetOf(this) : null;
});
reflectAll(HTMLStyleElement.prototype, ['media', 'type', ['disabled', 'disabled', 'bool']]);
reflectAll(HTMLMetaElement.prototype, ['name', 'content', ['httpEquiv', 'http-equiv'], 'charset', 'media', 'scheme']);
reflectAll(HTMLBaseElement.prototype, [['href', 'href', 'url'], 'target']);
reflectAll(HTMLIFrameElement.prototype, [['src', 'src', 'url'], 'srcdoc', 'name', 'width', 'height', 'allow',
  ['allowFullscreen', 'allowfullscreen', 'bool'], 'loading', ['referrerPolicy', 'referrerpolicy'], 'sandbox']);
// <iframe>: o quadro roda numa sessão própria; só passa postMessage (em JSON)
const packMessage = (data) => {
  try { return JSON.stringify({ v: data === undefined ? null : data }); } catch (e) { return '{"v":null}'; }
};
const unpackMessage = (s) => { try { return JSON.parse(s).v; } catch (e) { return null; } };
const FRAME_WIN = new WeakMap();
accessor(HTMLIFrameElement.prototype, 'contentWindow', function () {
  if (!this.isConnected) return null;
  let w = FRAME_WIN.get(this);
  if (!w) {
    const el = this;
    w = {
      postMessage(data, origin) {
        const target = origin && typeof origin === 'object' ? (origin.targetOrigin ?? '/') : String(origin ?? '*');
        N.framePost(el, packMessage(data), target === '/' ? location.origin : target);
      },
      get parent() { return G; }, get top() { return G.top; }, get opener() { return null; },
      get window() { return w; }, get self() { return w; }, get frames() { return w; },
      get location() { return { get href() { return N.frameUrl(el); }, replace(u) { el.src = u; }, assign(u) { el.src = u; } }; },
      get document() { return null; }, closed: false, length: 0, focus() {}, blur() {}, close() {},
    };
    FRAME_WIN.set(this, w);
  }
  return w;
});
accessor(HTMLIFrameElement.prototype, 'contentDocument', () => null);
// dentro de um quadro: parent/top falam com a página-mãe por mensagens
const PARENT_ORIGIN = N.frameParentOrigin();
if (PARENT_ORIGIN !== null) {
  const P = {
    postMessage(data, origin) {
      const target = origin && typeof origin === 'object' ? (origin.targetOrigin ?? '*') : String(origin ?? '*');
      N.parentPost(packMessage(data), target);
    },
    get parent() { return P; }, get top() { return P; }, get window() { return P; }, get self() { return P; },
    get opener() { return null; }, get location() { return { href: PARENT_ORIGIN + '/', origin: PARENT_ORIGIN }; },
    get document() { return null; }, get frames() { return P; }, closed: false, length: 0, focus() {}, blur() {},
  };
  hidden(G, 'parent', P);
  hidden(G, 'top', P);
  hidden(G, 'frameElement', null);
}
reflectAll(HTMLSourceElement.prototype, [['src', 'src', 'url'], 'srcset', 'sizes', 'media', 'type']);
reflectAll(HTMLTrackElement.prototype, [['src', 'src', 'url'], 'kind', 'srclang', 'label', ['default', 'default', 'bool']]);
reflectAll(HTMLEmbedElement.prototype, [['src', 'src', 'url'], 'type', 'width', 'height']);
reflectAll(HTMLObjectElement.prototype, [['data', 'data', 'url'], 'type', 'name', 'width', 'height']);
reflectAll(HTMLOListElement.prototype, [['start', 'start', 'int', 1], ['reversed', 'reversed', 'bool'], 'type']);
reflectAll(HTMLLIElement.prototype, [['value', 'value', 'int', 0]]);
reflectAll(HTMLTimeElement.prototype, [['dateTime', 'datetime']]);
reflectAll(HTMLDataElement.prototype, ['value']);
reflectAll(HTMLQuoteElement.prototype, [['cite', 'cite', 'url']]);
reflectAll(HTMLTableCellElement.prototype, [['colSpan', 'colspan', 'int', 1], ['rowSpan', 'rowspan', 'int', 1], 'headers', 'scope', 'abbr']);
reflectAll(HTMLProgressElement.prototype, [['max', 'max', 'int', 1], ['value', 'value', 'int', 0]]);
reflectAll(HTMLMeterElement.prototype, ['value', 'min', 'max', 'low', 'high', 'optimum']);
reflectAll(HTMLDialogElement.prototype, [['open', 'open', 'bool'], 'returnValue']);
methods(HTMLDialogElement.prototype, {
  show() { this.setAttribute('open', ''); },
  showModal() { this.setAttribute('open', ''); },
  close(v) {
    if (!this.hasAttribute('open')) return;
    if (v !== undefined) this.setAttribute('returnvalue', String(v));
    this.removeAttribute('open');
    this.dispatchEvent(new Event('close'));
  },
});
accessor(HTMLTableElement.prototype, 'rows', function () { return [...this.querySelectorAll('tr')].filter((r) => r.closest('table') === this); });
accessor(HTMLTableElement.prototype, 'tBodies', function () { return [...this.children].filter((c) => c.localName === 'tbody'); });
accessor(HTMLTableElement.prototype, 'tHead', function () { return [...this.children].find((c) => c.localName === 'thead') || null; });
accessor(HTMLTableElement.prototype, 'tFoot', function () { return [...this.children].find((c) => c.localName === 'tfoot') || null; });
accessor(HTMLTableElement.prototype, 'caption', function () { return [...this.children].find((c) => c.localName === 'caption') || null; });
accessor(HTMLTableSectionElement.prototype, 'rows', function () { return [...this.children].filter((c) => c.localName === 'tr'); });
accessor(HTMLTableRowElement.prototype, 'cells', function () { return [...this.children].filter((c) => c.localName === 'td' || c.localName === 'th'); });
accessor(HTMLTableRowElement.prototype, 'rowIndex', function () { const t = this.closest('table'); return t ? t.rows.indexOf(this) : -1; });
accessor(HTMLTableCellElement.prototype, 'cellIndex', function () { const r = this.parentElement; return r ? [...r.children].indexOf(this) : -1; });
methods(HTMLTableElement.prototype, {
  insertRow(i = -1) {
    let body = this.tBodies[0];
    if (!body) { body = document.createElement('tbody'); this.appendChild(body); }
    return HTMLTableSectionElement.prototype.insertRow.call(body, i);
  },
  deleteRow(i) { const r = this.rows[i < 0 ? this.rows.length + i : i]; if (r) r.remove(); },
  createTHead() { let h = this.tHead; if (!h) { h = document.createElement('thead'); this.prepend(h); } return h; },
  createTBody() { const b = document.createElement('tbody'); this.appendChild(b); return b; },
  createCaption() { let c = this.caption; if (!c) { c = document.createElement('caption'); this.prepend(c); } return c; },
});
methods(HTMLTableSectionElement.prototype, {
  insertRow(i = -1) {
    const r = document.createElement('tr');
    const rows = this.rows;
    if (i < 0 || i >= rows.length) this.appendChild(r); else rows[i].before(r);
    return r;
  },
  deleteRow(i) { const r = this.rows[i < 0 ? this.rows.length + i : i]; if (r) r.remove(); },
});
methods(HTMLTableRowElement.prototype, {
  insertCell(i = -1) {
    const c = document.createElement('td');
    const cells = this.cells;
    if (i < 0 || i >= cells.length) this.appendChild(c); else cells[i].before(c);
    return c;
  },
  deleteCell(i) { const c = this.cells[i < 0 ? this.cells.length + i : i]; if (c) c.remove(); },
});

// ---- canvas 2D: o estado do contexto fica aqui; o desenho, em Rust (script/canvas.rs)
const CTX2D = new WeakMap();
const TAU = Math.PI * 2;
const IDENT = () => [1, 0, 0, 1, 0, 0];
const mmul = (m, n) => [
  m[0] * n[0] + m[2] * n[1], m[1] * n[0] + m[3] * n[1],
  m[0] * n[2] + m[2] * n[3], m[1] * n[2] + m[3] * n[3],
  m[0] * n[4] + m[2] * n[5] + m[4], m[1] * n[4] + m[3] * n[5] + m[5],
];
const minv = (m) => {
  const det = m[0] * m[3] - m[1] * m[2];
  if (!det) return null;
  const i = 1 / det;
  return [m[3] * i, -m[1] * i, -m[2] * i, m[0] * i, (m[2] * m[5] - m[3] * m[4]) * i, (m[1] * m[4] - m[0] * m[5]) * i];
};
const fin = (...a) => a.every(Number.isFinite);

class DOMMatrixReadOnly {
  constructor(init) {
    let v = [1, 0, 0, 1, 0, 0];
    if (Array.isArray(init) && init.length === 6) v = init.map(Number);
    else if (Array.isArray(init) && init.length === 16) v = [init[0], init[1], init[4], init[5], init[12], init[13]].map(Number);
    else if (init && typeof init === 'object') v = [init.a ?? init.m11 ?? 1, init.b ?? init.m12 ?? 0, init.c ?? init.m21 ?? 0, init.d ?? init.m22 ?? 1, init.e ?? init.m41 ?? 0, init.f ?? init.m42 ?? 0];
    [this.a, this.b, this.c, this.d, this.e, this.f] = v;
  }
  get m11() { return this.a; } get m12() { return this.b; } get m21() { return this.c; } get m22() { return this.d; }
  get m41() { return this.e; } get m42() { return this.f; }
  get m13() { return 0; } get m14() { return 0; } get m23() { return 0; } get m24() { return 0; }
  get m31() { return 0; } get m32() { return 0; } get m33() { return 1; } get m34() { return 0; } get m43() { return 0; } get m44() { return 1; }
  get is2D() { return true; }
  get isIdentity() { return this.a === 1 && this.b === 0 && this.c === 0 && this.d === 1 && this.e === 0 && this.f === 0; }
  _v() { return [this.a, this.b, this.c, this.d, this.e, this.f]; }
  multiply(o) { return new DOMMatrix(mmul(this._v(), new DOMMatrix(o)._v())); }
  translate(x = 0, y = 0) { return new DOMMatrix(mmul(this._v(), [1, 0, 0, 1, x, y])); }
  scale(sx = 1, sy = sx) { return new DOMMatrix(mmul(this._v(), [sx, 0, 0, sy, 0, 0])); }
  rotate(deg = 0) { const r = deg * Math.PI / 180, c = Math.cos(r), s = Math.sin(r); return new DOMMatrix(mmul(this._v(), [c, s, -s, c, 0, 0])); }
  inverse() { return new DOMMatrix(minv(this._v()) || [NaN, NaN, NaN, NaN, NaN, NaN]); }
  transformPoint(p = {}) { const x = p.x ?? 0, y = p.y ?? 0; return { x: this.a * x + this.c * y + this.e, y: this.b * x + this.d * y + this.f, z: 0, w: 1 }; }
  toFloat32Array() { return new Float32Array([this.a, this.b, 0, 0, this.c, this.d, 0, 0, 0, 0, 1, 0, this.e, this.f, 0, 1]); }
  toFloat64Array() { return new Float64Array(this.toFloat32Array()); }
  toString() { return `matrix(${this._v().join(', ')})`; }
  static fromMatrix(o) { return new this(o); }
}
class DOMMatrix extends DOMMatrixReadOnly {
  multiplySelf(o) { [this.a, this.b, this.c, this.d, this.e, this.f] = this.multiply(o)._v(); return this; }
  preMultiplySelf(o) { [this.a, this.b, this.c, this.d, this.e, this.f] = mmul(new DOMMatrix(o)._v(), this._v()); return this; }
  translateSelf(x, y) { return this.multiplySelf(new DOMMatrix([1, 0, 0, 1, x || 0, y || 0])); }
  scaleSelf(sx = 1, sy = sx) { return this.multiplySelf(new DOMMatrix([sx, 0, 0, sy, 0, 0])); }
  rotateSelf(deg = 0) { [this.a, this.b, this.c, this.d, this.e, this.f] = this.rotate(deg)._v(); return this; }
  invertSelf() { [this.a, this.b, this.c, this.d, this.e, this.f] = this.inverse()._v(); return this; }
}
hidden(G, 'DOMMatrixReadOnly', DOMMatrixReadOnly);
hidden(G, 'DOMMatrix', DOMMatrix);
if (!G.WebKitCSSMatrix) hidden(G, 'WebKitCSSMatrix', DOMMatrix);

class CanvasGradient {
  constructor(type, args) { this._type = type; this._args = args; this._stops = []; }
  addColorStop(offset, color) {
    offset = Number(offset);
    if (!(offset >= 0 && offset <= 1)) throw new DOMException(`The provided value (${offset}) is outside the range (0.0, 1.0).`, 'IndexSizeError');
    if (N.cvColor(String(color)) === null) throw new DOMException(`The value provided ('${color}') could not be parsed as a color.`, 'SyntaxError');
    this._stops.push([offset, String(color)]);
  }
}
class CanvasPattern {
  constructor(img, rep) { this._type = 'pattern'; this._img = img; this._rep = rep; this._args = []; }
  setTransform() {}
}
hidden(G, 'CanvasGradient', CanvasGradient);
hidden(G, 'CanvasPattern', CanvasPattern);

class ImageData {
  constructor(a, b, c) {
    let data, w, h;
    if (a instanceof Uint8ClampedArray) {
      data = a; w = b >>> 0; h = c === undefined ? a.length / 4 / w : c >>> 0;
      if (!w || a.length !== w * h * 4) throw new DOMException('The input data length is not equal to (4 * width * height).', 'IndexSizeError');
    } else {
      w = a >>> 0; h = b >>> 0;
      if (!w || !h) throw new DOMException('The source width is 0.', 'IndexSizeError');
      data = new Uint8ClampedArray(w * h * 4);
    }
    Object.defineProperty(this, 'data', { value: data, enumerable: true });
    Object.defineProperty(this, 'width', { value: w, enumerable: true });
    Object.defineProperty(this, 'height', { value: h, enumerable: true });
    this.colorSpace = 'srgb';
  }
}
hidden(G, 'ImageData', ImageData);

class TextMetrics {}
hidden(G, 'TextMetrics', TextMetrics);

class ImageBitmap {
  constructor(id, w, h) { this._source = id; this.width = w; this.height = h; }
  close() { if (this._source) N.bitmapClose(this._source); this._source = 0; this.width = this.height = 0; }
}
hidden(G, 'ImageBitmap', ImageBitmap);
hidden(G, 'createImageBitmap', async function createImageBitmap(src) {
  let r = null;
  if (src instanceof Blob) r = N.bitmapDecode(await src.bytes());
  else if (src instanceof ImageBitmap) r = [src._source, src.width, src.height];
  else {
    if (src && src.localName === 'img' && !src.complete && src.decode) await src.decode().catch(() => {});
    r = N.bitmapNew(src && src._el ? src._el : src);
  }
  if (!r) throw new DOMException('The source image could not be decoded.', 'InvalidStateError');
  return new ImageBitmap(r[0], r[1], r[2]);
});

// Caminho: comandos em coordenadas do usuário (Path2D) ou do dispositivo (contexto).
class PathBuilder {
  constructor() { this.d = []; this.last = null; this.start = null; }
  clone() { const p = new PathBuilder(); p.d = this.d.slice(); p.last = this.last; p.start = this.start; return p; }
  moveTo(m, x, y) {
    if (!fin(x, y)) return;
    const p = [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
    this.d.push(0, p[0], p[1]); this.last = this.start = p;
  }
  lineTo(m, x, y) {
    if (!fin(x, y)) return;
    if (!this.last) return this.moveTo(m, x, y);
    const p = [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
    this.d.push(1, p[0], p[1]); this.last = p;
  }
  bezierCurveTo(m, a, b, c, d, x, y) {
    if (!fin(a, b, c, d, x, y)) return;
    if (!this.last) this.moveTo(m, a, b);
    const T = (u, v) => [m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]];
    const p1 = T(a, b), p2 = T(c, d), p3 = T(x, y);
    this.d.push(2, p1[0], p1[1], p2[0], p2[1], p3[0], p3[1]); this.last = p3;
  }
  // ponto atual no espaço do usuário de agora
  userLast(m) {
    if (!this.last) return null;
    const i = minv(m) || IDENT();
    const [x, y] = this.last;
    return [i[0] * x + i[2] * y + i[4], i[1] * x + i[3] * y + i[5]];
  }
  quadraticCurveTo(m, cx, cy, x, y) {
    if (!fin(cx, cy, x, y)) return;
    if (!this.last) this.moveTo(m, cx, cy);
    const [x0, y0] = this.userLast(m);
    this.bezierCurveTo(m, x0 + 2 / 3 * (cx - x0), y0 + 2 / 3 * (cy - y0), x + 2 / 3 * (cx - x), y + 2 / 3 * (cy - y), x, y);
  }
  closePath() { if (this.last) { this.d.push(3); this.last = this.start; } }
  rect(m, x, y, w, h) {
    if (!fin(x, y, w, h)) return;
    this.moveTo(m, x, y); this.lineTo(m, x + w, y); this.lineTo(m, x + w, y + h); this.lineTo(m, x, y + h); this.closePath();
  }
  ellipse(m, x, y, rx, ry, rot, a0, a1, ccw) {
    if (!fin(x, y, rx, ry, rot, a0, a1)) return;
    if (rx < 0 || ry < 0) throw new DOMException(`The radius provided (${rx < 0 ? rx : ry}) is negative.`, 'IndexSizeError');
    let sweep = a1 - a0;
    if (!ccw) { if (sweep >= TAU) sweep = TAU; else { sweep %= TAU; if (sweep < 0) sweep += TAU; } }
    else { if (sweep <= -TAU) sweep = -TAU; else { sweep %= TAU; if (sweep > 0) sweep -= TAU; } }
    const cr = Math.cos(rot), sr = Math.sin(rot);
    const E = (u, v) => [x + rx * u * cr - ry * v * sr, y + rx * u * sr + ry * v * cr];
    const s = E(Math.cos(a0), Math.sin(a0));
    if (this.last) this.lineTo(m, s[0], s[1]); else this.moveTo(m, s[0], s[1]);
    if (sweep === 0) return;
    const n = Math.max(1, Math.ceil(Math.abs(sweep) / (Math.PI / 2) - 1e-9));
    const th = sweep / n, k = 4 / 3 * Math.tan(th / 4);
    let t = a0;
    for (let i = 0; i < n; i++) {
      const c0 = Math.cos(t), s0 = Math.sin(t), c1 = Math.cos(t + th), s1 = Math.sin(t + th);
      const p1 = E(c0 - k * s0, s0 + k * c0), p2 = E(c1 + k * s1, s1 - k * c1), p3 = E(c1, s1);
      this.bezierCurveTo(m, p1[0], p1[1], p2[0], p2[1], p3[0], p3[1]);
      t += th;
    }
  }
  arcTo(m, x1, y1, x2, y2, r) {
    if (!fin(x1, y1, x2, y2, r)) return;
    if (r < 0) throw new DOMException(`The radius provided (${r}) is negative.`, 'IndexSizeError');
    if (!this.last) this.moveTo(m, x1, y1);
    const [x0, y0] = this.userLast(m);
    const v1x = x0 - x1, v1y = y0 - y1, v2x = x2 - x1, v2y = y2 - y1;
    const l1 = Math.hypot(v1x, v1y), l2 = Math.hypot(v2x, v2y);
    const cross = (v1x * v2y - v1y * v2x) / (l1 * l2 || 1);
    if (!l1 || !l2 || !r || Math.abs(cross) < 1e-9) return this.lineTo(m, x1, y1);
    const ux = v1x / l1, uy = v1y / l1, wx = v2x / l2, wy = v2y / l2;
    const cos = Math.max(-1, Math.min(1, ux * wx + uy * wy)), th = Math.acos(cos);
    const t = r / Math.tan(th / 2);
    const t1 = [x1 + ux * t, y1 + uy * t], t2 = [x1 + wx * t, y1 + wy * t];
    const bx = ux + wx, by = uy + wy, bl = Math.hypot(bx, by);
    const d = r / Math.sin(th / 2);
    const cx = x1 + bx / bl * d, cy = y1 + by / bl * d;
    this.lineTo(m, t1[0], t1[1]);
    this.ellipse(m, cx, cy, r, r, 0, Math.atan2(t1[1] - cy, t1[0] - cx), Math.atan2(t2[1] - cy, t2[0] - cx), cross > 0);
  }
  roundRect(m, x, y, w, h, radii = 0) {
    if (!fin(x, y, w, h)) return;
    let rs = Array.isArray(radii) ? radii : [radii];
    if (rs.length < 1 || rs.length > 4) throw new RangeError(`${rs.length} radii provided. Between one and four radii are necessary.`);
    rs = rs.map((r) => (typeof r === 'object' ? [Number(r.x) || 0, Number(r.y) || 0] : [Number(r) || 0, Number(r) || 0]));
    if (rs.some(([a, b]) => a < 0 || b < 0)) throw new RangeError('Radius value is negative.');
    const [tl, tr, br, bl] = rs.length === 1 ? [rs[0], rs[0], rs[0], rs[0]] : rs.length === 2 ? [rs[0], rs[1], rs[0], rs[1]] : rs.length === 3 ? [rs[0], rs[1], rs[2], rs[1]] : rs;
    const aw = Math.abs(w), ah = Math.abs(h);
    const f = Math.min(1, aw / (tl[0] + tr[0] || 1), aw / (bl[0] + br[0] || 1), ah / (tl[1] + bl[1] || 1), ah / (tr[1] + br[1] || 1));
    const R = (r) => [r[0] * f, r[1] * f];
    const [a, b, c, d] = [R(tl), R(tr), R(br), R(bl)];
    const sx = Math.sign(w) || 1, sy = Math.sign(h) || 1;
    this.moveTo(m, x + a[0] * sx, y);
    this.lineTo(m, x + w - b[0] * sx, y);
    this.ellipse(m, x + w - b[0] * sx, y + b[1] * sy, b[0], b[1], 0, -Math.PI / 2, 0, false);
    this.lineTo(m, x + w, y + h - c[1] * sy);
    this.ellipse(m, x + w - c[0] * sx, y + h - c[1] * sy, c[0], c[1], 0, 0, Math.PI / 2, false);
    this.lineTo(m, x + d[0] * sx, y + h);
    this.ellipse(m, x + d[0] * sx, y + h - d[1] * sy, d[0], d[1], 0, Math.PI / 2, Math.PI, false);
    this.lineTo(m, x, y + a[1] * sy);
    this.ellipse(m, x + a[0] * sx, y + a[1] * sy, a[0], a[1], 0, Math.PI, Math.PI * 1.5, false);
    this.closePath();
    this.moveTo(m, x, y);
  }
  // acrescenta comandos já prontos (Path2D de texto SVG), transformados por m
  addCommands(m, d) {
    const T = (u, v) => [m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]];
    for (let i = 0; i < d.length;) {
      const op = d[i];
      if (op === 0 || op === 1) { const p = T(d[i + 1], d[i + 2]); this.d.push(op, p[0], p[1]); this.last = p; if (op === 0) this.start = p; i += 3; }
      else if (op === 2) { const a = T(d[i + 1], d[i + 2]), b = T(d[i + 3], d[i + 4]), c = T(d[i + 5], d[i + 6]); this.d.push(2, a[0], a[1], b[0], b[1], c[0], c[1]); this.last = c; i += 7; }
      else { this.d.push(3); this.last = this.start; i += 1; }
    }
  }
}

// Path2D guarda as operações (no espaço do usuário) e as repete com a
// transformação de quem desenha.
const PATH_OPS = new WeakMap();
class Path2D {
  constructor(src) {
    const ops = [];
    PATH_OPS.set(this, ops);
    if (src instanceof Path2D) ops.push(...PATH_OPS.get(src));
    else if (typeof src === 'string') ops.push(['addCommands', [Array.from(N.cvSvgPath(src))]]);
  }
  addPath(p, t) {
    if (!(p instanceof Path2D)) throw new TypeError("Failed to execute 'addPath' on 'Path2D': parameter 1 is not of type 'Path2D'.");
    const m = t ? new DOMMatrix(t)._v() : null;
    PATH_OPS.get(this).push(['_sub', [PATH_OPS.get(p).slice(), m]]);
  }
}
for (const k of ['moveTo', 'lineTo', 'bezierCurveTo', 'quadraticCurveTo', 'closePath', 'rect', 'arc', 'ellipse', 'arcTo', 'roundRect']) {
  hidden(Path2D.prototype, k, function (...a) { PATH_OPS.get(this).push([k, a]); });
}
hidden(G, 'Path2D', Path2D);

// repete as operações de um Path2D num PathBuilder
function replayPath(pb, ops, m) {
  for (const [k, a] of ops) {
    if (k === '_sub') { replayPath(pb, a[0], a[1] ? mmul(m, a[1]) : m); continue; }
    if (k === 'closePath') pb.closePath();
    else if (k === 'arc') pb.ellipse(m, a[0], a[1], a[2], a[2], 0, a[3], a[4], a[5]);
    else if (k === 'addCommands') pb.addCommands(m, a[0]);
    else pb[k](m, ...a);
  }
}

const CTX_DEFAULTS = () => ({
  fill: '#000000', stroke: '#000000', lineWidth: 1, lineCap: 'butt', lineJoin: 'miter', miterLimit: 10,
  dash: [], dashOffset: 0, alpha: 1, op: 'source-over', font: '10px sans-serif', textAlign: 'start',
  textBaseline: 'alphabetic', direction: 'inherit', smoothing: true, smoothingQuality: 'low', m: IDENT(),
  shadowBlur: 0, shadowColor: 'rgba(0, 0, 0, 0)', shadowOffsetX: 0, shadowOffsetY: 0, filter: 'none',
  letterSpacing: '0px', wordSpacing: '0px', fontKerning: 'auto', fontStretch: 'normal', fontVariantCaps: 'normal',
  textRendering: 'auto',
});

class CanvasRenderingContext2D {
  constructor(el, canvas) {
    this._el = el; this._canvas = canvas || el;
    this._s = CTX_DEFAULTS(); this._stack = []; this._path = new PathBuilder();
  }
  get canvas() { return this._canvas; }
  getContextAttributes() { return { alpha: true, colorSpace: 'srgb', desynchronized: false, willReadFrequently: false }; }
  isContextLost() { return false; }
  // estado
  save() { const s = this._s; this._stack.push({ ...s, dash: s.dash.slice(), m: s.m.slice() }); N.cvSave(this._el); }
  restore() { if (this._stack.length) { this._s = this._stack.pop(); N.cvRestore(this._el); } }
  reset() {
    this._s = CTX_DEFAULTS(); this._stack = []; this._path = new PathBuilder();
    N.cvResetState(this._el); N.cvSize(this._el, this._canvas.width, this._canvas.height);
  }
  _resetState() { this._s = CTX_DEFAULTS(); this._stack = []; this._path = new PathBuilder(); N.cvResetState(this._el); }
  // transformação
  scale(x, y) { if (fin(x, y)) this._s.m = mmul(this._s.m, [x, 0, 0, y, 0, 0]); }
  rotate(a) { if (fin(a)) { const c = Math.cos(a), s = Math.sin(a); this._s.m = mmul(this._s.m, [c, s, -s, c, 0, 0]); } }
  translate(x, y) { if (fin(x, y)) this._s.m = mmul(this._s.m, [1, 0, 0, 1, x, y]); }
  transform(a, b, c, d, e, f) { if (fin(a, b, c, d, e, f)) this._s.m = mmul(this._s.m, [a, b, c, d, e, f]); }
  setTransform(a, b, c, d, e, f) {
    if (a === undefined) return this.resetTransform();
    if (typeof a === 'object') { const v = new DOMMatrix(a)._v(); if (fin(...v)) this._s.m = v; return; }
    if (fin(a, b, c, d, e, f)) this._s.m = [a, b, c, d, e, f];
  }
  resetTransform() { this._s.m = IDENT(); }
  getTransform() { return new DOMMatrix(this._s.m); }
  // estilos
  get fillStyle() { return this._s.fill; }
  set fillStyle(v) { this._s.fill = this._style(v, this._s.fill); }
  get strokeStyle() { return this._s.stroke; }
  set strokeStyle(v) { this._s.stroke = this._style(v, this._s.stroke); }
  _style(v, old) {
    if (v instanceof CanvasGradient || v instanceof CanvasPattern) return v;
    const c = N.cvColor(String(v));
    return c === null ? old : c;
  }
  get globalAlpha() { return this._s.alpha; }
  set globalAlpha(v) { v = Number(v); if (v >= 0 && v <= 1) this._s.alpha = v; }
  get globalCompositeOperation() { return this._s.op; }
  set globalCompositeOperation(v) {
    const ok = ['source-over', 'source-in', 'source-out', 'source-atop', 'destination-over', 'destination-in', 'destination-out',
      'destination-atop', 'lighter', 'copy', 'xor', 'multiply', 'screen', 'overlay', 'darken', 'lighten', 'color-dodge',
      'color-burn', 'hard-light', 'soft-light', 'difference', 'exclusion', 'hue', 'saturation', 'color', 'luminosity'];
    if (ok.includes(String(v))) this._s.op = String(v);
  }
  get lineWidth() { return this._s.lineWidth; }
  set lineWidth(v) { v = Number(v); if (v > 0 && Number.isFinite(v)) this._s.lineWidth = v; }
  get lineCap() { return this._s.lineCap; }
  set lineCap(v) { if (['butt', 'round', 'square'].includes(v)) this._s.lineCap = v; }
  get lineJoin() { return this._s.lineJoin; }
  set lineJoin(v) { if (['round', 'bevel', 'miter'].includes(v)) this._s.lineJoin = v; }
  get miterLimit() { return this._s.miterLimit; }
  set miterLimit(v) { v = Number(v); if (v > 0 && Number.isFinite(v)) this._s.miterLimit = v; }
  get lineDashOffset() { return this._s.dashOffset; }
  set lineDashOffset(v) { v = Number(v); if (Number.isFinite(v)) this._s.dashOffset = v; }
  setLineDash(list) {
    const l = Array.from(list, Number);
    if (l.some((x) => !(x >= 0) || !Number.isFinite(x))) return;
    this._s.dash = l.length % 2 ? l.concat(l) : l;
  }
  getLineDash() { return this._s.dash.slice(); }
  get font() { return this._s.font; }
  set font(v) { if (String(v).trim()) this._s.font = String(v); }
  get textAlign() { return this._s.textAlign; }
  set textAlign(v) { if (['start', 'end', 'left', 'right', 'center'].includes(v)) this._s.textAlign = v; }
  get textBaseline() { return this._s.textBaseline; }
  set textBaseline(v) { if (['top', 'hanging', 'middle', 'alphabetic', 'ideographic', 'bottom'].includes(v)) this._s.textBaseline = v; }
  get direction() { return this._s.direction; }
  set direction(v) { if (['ltr', 'rtl', 'inherit'].includes(v)) this._s.direction = v; }
  get imageSmoothingEnabled() { return this._s.smoothing; }
  set imageSmoothingEnabled(v) { this._s.smoothing = !!v; }
  get imageSmoothingQuality() { return this._s.smoothingQuality; }
  set imageSmoothingQuality(v) { if (['low', 'medium', 'high'].includes(v)) this._s.smoothingQuality = v; }
  // caminho atual
  beginPath() { this._path = new PathBuilder(); }
  closePath() { this._path.closePath(); }
  moveTo(x, y) { this._path.moveTo(this._s.m, +x, +y); }
  lineTo(x, y) { this._path.lineTo(this._s.m, +x, +y); }
  bezierCurveTo(a, b, c, d, x, y) { this._path.bezierCurveTo(this._s.m, +a, +b, +c, +d, +x, +y); }
  quadraticCurveTo(a, b, x, y) { this._path.quadraticCurveTo(this._s.m, +a, +b, +x, +y); }
  rect(x, y, w, h) { this._path.rect(this._s.m, +x, +y, +w, +h); }
  roundRect(x, y, w, h, r) { this._path.roundRect(this._s.m, +x, +y, +w, +h, r); }
  arc(x, y, r, a0, a1, ccw = false) { this._path.ellipse(this._s.m, +x, +y, +r, +r, 0, +a0, +a1, !!ccw); }
  ellipse(x, y, rx, ry, rot, a0, a1, ccw = false) { this._path.ellipse(this._s.m, +x, +y, +rx, +ry, +rot, +a0, +a1, !!ccw); }
  arcTo(x1, y1, x2, y2, r) { this._path.arcTo(this._s.m, +x1, +y1, +x2, +y2, +r); }
  // caminho para desenhar: o atual ou um Path2D (com a transformação de agora)
  _pathOf(p) {
    if (p instanceof Path2D) { const pb = new PathBuilder(); replayPath(pb, PATH_OPS.get(p), this._s.m); return pb.d; }
    return this._path.d;
  }
  _args(a) {
    let p = null, rule = 'nonzero';
    if (a[0] instanceof Path2D) { p = a[0]; rule = a[1] ?? 'nonzero'; } else rule = a[0] ?? 'nonzero';
    return [new Float64Array(this._pathOf(p)), rule === 'evenodd'];
  }
  fill(...a) { const [d, eo] = this._args(a); N.cvFill(this._el, d, eo, this._s.fill, this._s.m, this._s.alpha, this._s.op); }
  _strokePath(d, style) {
    const s = this._s, k = Math.sqrt(Math.abs(s.m[0] * s.m[3] - s.m[1] * s.m[2])) || 1;
    N.cvStroke(this._el, d, style, s.m, s.alpha, s.op, s.lineWidth * k, s.lineCap, s.lineJoin, s.miterLimit,
      s.dash.map((x) => x * k), s.dashOffset * k);
  }
  stroke(p) { this._strokePath(new Float64Array(this._pathOf(p instanceof Path2D ? p : null)), this._s.stroke); }
  clip(...a) { const [d, eo] = this._args(a); N.cvClip(this._el, d, eo); }
  isPointInPath(...a) {
    let p = null;
    if (a[0] instanceof Path2D) p = a.shift();
    const [x, y, rule = 'nonzero'] = a;
    const d = p ? (() => { const pb = new PathBuilder(); replayPath(pb, PATH_OPS.get(p), IDENT()); return pb.d; })() : this._path.d;
    // os pontos do caminho atual já estão no espaço do dispositivo
    return pointInPath(d, +x, +y, rule === 'evenodd');
  }
  isPointInStroke() { return false; }
  // retângulos
  _rectPath(x, y, w, h) { const pb = new PathBuilder(); pb.rect(this._s.m, +x, +y, +w, +h); return new Float64Array(pb.d); }
  fillRect(x, y, w, h) { if (fin(+x, +y, +w, +h)) N.cvFill(this._el, this._rectPath(x, y, w, h), false, this._s.fill, this._s.m, this._s.alpha, this._s.op); }
  strokeRect(x, y, w, h) { if (fin(+x, +y, +w, +h)) this._strokePath(this._rectPath(x, y, w, h), this._s.stroke); }
  clearRect(x, y, w, h) {
    [x, y, w, h] = [+x, +y, +w, +h];
    if (!fin(x, y, w, h)) return;
    const m = this._s.m;
    if (m[1] === 0 && m[2] === 0) {
      const x0 = m[0] * x + m[4], y0 = m[3] * y + m[5], x1 = m[0] * (x + w) + m[4], y1 = m[3] * (y + h) + m[5];
      N.cvClear(this._el, Math.min(x0, x1), Math.min(y0, y1), Math.abs(x1 - x0), Math.abs(y1 - y0));
    } else N.cvFill(this._el, this._rectPath(x, y, w, h), false, '#000', m, 1, 'destination-out');
  }
  // texto
  _align() {
    const a = this._s.textAlign, rtl = this._s.direction === 'rtl';
    return a === 'start' ? (rtl ? 'right' : 'left') : a === 'end' ? (rtl ? 'left' : 'right') : a;
  }
  fillText(t, x, y, maxW) {
    if (!fin(+x, +y)) return;
    N.cvText(this._el, String(t), +x, +y, this._s.font, this._align(), this._s.textBaseline, this._s.fill, this._s.m, this._s.alpha, this._s.op, maxW === undefined ? Infinity : +maxW);
  }
  strokeText(t, x, y, maxW) {
    if (!fin(+x, +y)) return;
    N.cvText(this._el, String(t), +x, +y, this._s.font, this._align(), this._s.textBaseline, this._s.stroke, this._s.m, this._s.alpha, this._s.op, maxW === undefined ? Infinity : +maxW);
  }
  measureText(t) {
    const [w, asc, desc, fa, fd] = N.cvMeasure(this._s.font, String(t));
    const m = new TextMetrics();
    const align = this._align();
    const left = align === 'center' ? w / 2 : align === 'right' ? w : 0;
    Object.assign(m, {
      width: w, actualBoundingBoxLeft: left, actualBoundingBoxRight: w - left,
      actualBoundingBoxAscent: asc, actualBoundingBoxDescent: desc,
      fontBoundingBoxAscent: fa, fontBoundingBoxDescent: fd, emHeightAscent: fa, emHeightDescent: fd,
      hangingBaseline: fa * 0.8, alphabeticBaseline: 0, ideographicBaseline: -fd,
    });
    return m;
  }
  // imagens
  drawImage(img, ...a) {
    if (!img) throw new TypeError("Failed to execute 'drawImage': The image argument is null.");
    const src = img._el || img;
    let sw, sh;
    if (img instanceof ImageBitmap) [sw, sh] = [img.width, img.height];
    else [sw, sh] = N.cvSourceSize(src);
    if (!sw || !sh) return;
    let sx = 0, sy = 0, dx, dy, dw = sw, dh = sh;
    if (a.length === 2) [dx, dy] = a;
    else if (a.length === 4) [dx, dy, dw, dh] = a;
    else if (a.length === 8) [sx, sy, sw, sh, dx, dy, dw, dh] = a;
    else throw new TypeError("Failed to execute 'drawImage': Valid arities are: [3, 5, 9].");
    if (!fin(+sx, +sy, +sw, +sh, +dx, +dy, +dw, +dh)) return;
    N.cvImage(this._el, img instanceof ImageBitmap ? img : src, +sx, +sy, +sw, +sh, +dx, +dy, +dw, +dh, this._s.m, this._s.alpha, this._s.op, this._s.smoothing);
  }
  createLinearGradient(x0, y0, x1, y1) { return new CanvasGradient('linear', [+x0, +y0, +x1, +y1]); }
  createRadialGradient(x0, y0, r0, x1, y1, r1) {
    if (r0 < 0 || r1 < 0) throw new DOMException(`The ${r0 < 0 ? 'r0' : 'r1'} provided is less than 0.`, 'IndexSizeError');
    return new CanvasGradient('radial', [+x0, +y0, +r0, +x1, +y1, +r1]);
  }
  createConicGradient(a, x, y) { return new CanvasGradient('conic', [+a, +x, +y]); }
  createPattern(img, rep) {
    rep = rep === null || rep === undefined || rep === '' ? 'repeat' : String(rep);
    if (!['repeat', 'repeat-x', 'repeat-y', 'no-repeat'].includes(rep)) throw new DOMException(`The provided type ('${rep}') is not one of 'repeat', 'repeat-x', 'repeat-y', or 'no-repeat'.`, 'SyntaxError');
    return new CanvasPattern(img instanceof ImageBitmap ? img : (img._el || img), rep);
  }
  // pixels
  createImageData(w, h) { return w instanceof ImageData ? new ImageData(w.width, w.height) : new ImageData(Math.abs(w), Math.abs(h)); }
  getImageData(x, y, w, h) {
    [x, y, w, h] = [x | 0, y | 0, w | 0, h | 0];
    if (!w || !h) throw new DOMException(`The source ${w ? 'height' : 'width'} is 0.`, 'IndexSizeError');
    if (w < 0) { x += w; w = -w; }
    if (h < 0) { y += h; h = -h; }
    return new ImageData(N.cvGetData(this._el, x, y, w, h), w, h);
  }
  putImageData(img, dx, dy, x = 0, y = 0, w = img.width, h = img.height) {
    if (w < 0) { x += w; w = -w; }
    if (h < 0) { y += h; h = -h; }
    N.cvPutData(this._el, img.data, img.width, img.height, dx | 0, dy | 0, x | 0, y | 0, w | 0, h | 0);
  }
  drawFocusIfNeeded() {}
  scrollPathIntoView() {}
}
// sombras e filtros: aceitos, mas ainda não desenhados
for (const k of ['shadowBlur', 'shadowColor', 'shadowOffsetX', 'shadowOffsetY', 'filter', 'letterSpacing', 'wordSpacing',
  'fontKerning', 'fontStretch', 'fontVariantCaps', 'textRendering']) {
  Object.defineProperty(CanvasRenderingContext2D.prototype, k, {
    get() { return this._s[k]; }, set(v) { this._s[k] = typeof this._s[k] === 'number' ? Number(v) : String(v); }, configurable: true,
  });
}
hidden(G, 'CanvasRenderingContext2D', CanvasRenderingContext2D);

// ponto dentro do caminho (polígono aproximado, curvas achatadas)
function pointInPath(d, x, y, evenodd) {
  const pts = [];
  let cur = [], start = null, last = null;
  const flush = () => { if (cur.length > 2) pts.push(cur); cur = []; };
  for (let i = 0; i < d.length;) {
    const op = d[i];
    if (op === 0) { flush(); last = start = [d[i + 1], d[i + 2]]; cur.push(last); i += 3; }
    else if (op === 1) { last = [d[i + 1], d[i + 2]]; cur.push(last); i += 3; }
    else if (op === 2) {
      const [x0, y0] = last || [d[i + 1], d[i + 2]];
      for (let k = 1; k <= 8; k++) {
        const t = k / 8, u = 1 - t;
        cur.push([u * u * u * x0 + 3 * u * u * t * d[i + 1] + 3 * u * t * t * d[i + 3] + t * t * t * d[i + 5],
          u * u * u * y0 + 3 * u * u * t * d[i + 2] + 3 * u * t * t * d[i + 4] + t * t * t * d[i + 6]]);
      }
      last = [d[i + 5], d[i + 6]]; i += 7;
    } else { flush(); last = start; if (start) cur.push(start); i += 1; }
  }
  flush();
  let wind = 0;
  for (const poly of pts) {
    for (let i = 0; i < poly.length; i++) {
      const [ax, ay] = poly[i], [bx, by] = poly[(i + 1) % poly.length];
      if ((ay <= y) !== (by <= y)) {
        const xi = ax + (y - ay) * (bx - ax) / (by - ay);
        if (xi > x) wind += by > ay ? 1 : -1;
      }
    }
  }
  return evenodd ? wind % 2 !== 0 : wind !== 0;
}

const CANVAS = HTMLCanvasElement.prototype;
for (const [k, def] of [['width', 300], ['height', 150]]) {
  Object.defineProperty(CANVAS, k, {
    get() { const v = parseInt(this.getAttribute(k), 10); return Number.isFinite(v) && v >= 0 ? v : def; },
    set(v) {
      v = Math.max(0, Math.trunc(Number(v)) || 0);
      this.setAttribute(k, String(v));
      const c = CTX2D.get(this);
      if (c) c._resetState();
    },
    configurable: true, enumerable: true,
  });
}
methods(CANVAS, {
  getContext(type) {
    type = String(type);
    if (type !== '2d') return null;
    let c = CTX2D.get(this);
    if (!c) {
      c = new CanvasRenderingContext2D(this);
      CTX2D.set(this, c);
      N.cvSize(this, this.width, this.height);
    }
    return c;
  },
  toDataURL() {
    if (!CTX2D.get(this)) this.getContext('2d');
    return N.cvDataURL(this);
  },
  toBlob(cb, type) {
    const url = this.toDataURL(type);
    setTimeout(() => {
      if (url === 'data:,') return cb(null);
      const bin = atob(url.slice(url.indexOf(',') + 1));
      const bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
      cb(new Blob([bytes], { type: 'image/png' }));
    }, 0);
  },
  captureStream() { return null; },
  transferControlToOffscreen() { throw new DOMException('transferControlToOffscreen ainda não é suportado.', 'NotSupportedError'); },
});

class OffscreenCanvas {
  constructor(w, h) { this._el = document.createElement('canvas'); this._el.width = w; this._el.height = h; }
  get width() { return this._el.width; }
  set width(v) { this._el.width = v; }
  get height() { return this._el.height; }
  set height(v) { this._el.height = v; }
  getContext(type) {
    if (String(type) !== '2d') return null;
    const c = this._el.getContext('2d');
    c._canvas = this;
    return c;
  }
  convertToBlob() { return new Promise((res) => this._el.toBlob(res)); }
  transferToImageBitmap() { const r = N.bitmapNew(this._el); return r ? new ImageBitmap(r[0], r[1], r[2]) : null; }
}
hidden(G, 'OffscreenCanvas', OffscreenCanvas);
hidden(G, 'OffscreenCanvasRenderingContext2D', CanvasRenderingContext2D);

const MEDIA = HTMLMediaElement.prototype;
reflectAll(MEDIA, [['src', 'src', 'url'], ['autoplay', 'autoplay', 'bool'], ['loop', 'loop', 'bool'],
  ['controls', 'controls', 'bool'], ['defaultMuted', 'muted', 'bool'], 'preload', ['crossOrigin', 'crossorigin']]);
reflectAll(HTMLVideoElement.prototype, [['poster', 'poster', 'url'], ['playsInline', 'playsinline', 'bool'],
  ['width', 'width', 'int', 0], ['height', 'height', 'int', 0]]);
// ---- mídia: Media Source Extensions + elementos <video>/<audio> (relógio em Rust)
class TimeRanges {
  constructor(flat) { this._r = flat || []; }
  get length() { return this._r.length / 2; }
  start(i) {
    if (!(i >= 0 && i < this.length)) throw new DOMException(`Failed to execute 'start' on 'TimeRanges': The index provided (${i}) is greater than or equal to the maximum bound (${this.length}).`, 'IndexSizeError');
    return this._r[2 * i];
  }
  end(i) {
    if (!(i >= 0 && i < this.length)) throw new DOMException(`Failed to execute 'end' on 'TimeRanges': The index provided (${i}) is greater than or equal to the maximum bound (${this.length}).`, 'IndexSizeError');
    return this._r[2 * i + 1];
  }
}
hidden(G, 'TimeRanges', TimeRanges);
class MediaError {
  constructor(code, message) { this.code = code; this.message = message || ''; }
}
for (const [k, v] of [['MEDIA_ERR_ABORTED', 1], ['MEDIA_ERR_NETWORK', 2], ['MEDIA_ERR_DECODE', 3], ['MEDIA_ERR_SRC_NOT_SUPPORTED', 4]]) {
  MediaError[k] = v; MediaError.prototype[k] = v;
}
hidden(G, 'MediaError', MediaError);

const MS = new WeakMap();
const SB = new WeakMap();
const later = (f) => setTimeout(f, 0);
class SourceBuffer extends EventTarget {
  constructor() { throw new TypeError('Illegal constructor'); }
  get updating() { return SB.get(this).updating; }
  get buffered() { return new TimeRanges(N.sbBuffered(SB.get(this).id)); }
  get mode() { return SB.get(this).mode; }
  set mode(v) { SB.get(this).mode = String(v); }
  get timestampOffset() { return SB.get(this).offset; }
  set timestampOffset(v) { SB.get(this).offset = Number(v) || 0; }
  get appendWindowStart() { return SB.get(this).winStart; }
  set appendWindowStart(v) { SB.get(this).winStart = +v; }
  get appendWindowEnd() { return SB.get(this).winEnd; }
  set appendWindowEnd(v) { SB.get(this).winEnd = +v; }
  get audioTracks() { return []; }
  get videoTracks() { return []; }
  get textTracks() { return []; }
  appendBuffer(data) {
    const s = SB.get(this);
    if (s.updating) throw new DOMException("Failed to execute 'appendBuffer' on 'SourceBuffer': This SourceBuffer is still processing an 'appendBuffer' or 'remove' operation.", 'InvalidStateError');
    if (s.removed) throw new DOMException("Failed to execute 'appendBuffer' on 'SourceBuffer': This SourceBuffer has been removed from the parent media source.", 'InvalidStateError');
    const bytes = ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : new Uint8Array(data);
    s.updating = true;
    const ms = s.ms;
    if (MS.get(ms).readyState === 'ended') { MS.get(ms).readyState = 'open'; }
    later(() => {
      fireTrusted(this, new Event('updatestart'));
      const err = N.sbAppend(s.id, bytes, s.offset);
      s.updating = false;
      if (err) {
        N.log('SourceBuffer: ' + err);
        fireTrusted(this, new Event('error'));
      } else {
        fireTrusted(this, new Event('update'));
      }
      fireTrusted(this, new Event('updateend'));
    });
  }
  remove(start, end) {
    const s = SB.get(this);
    if (s.updating) throw new DOMException("Failed to execute 'remove' on 'SourceBuffer': This SourceBuffer is still processing an 'appendBuffer' or 'remove' operation.", 'InvalidStateError');
    s.updating = true;
    later(() => {
      fireTrusted(this, new Event('updatestart'));
      N.sbRemove(s.id, +start, +end);
      s.updating = false;
      fireTrusted(this, new Event('update'));
      fireTrusted(this, new Event('updateend'));
    });
  }
  abort() { SB.get(this).updating = false; }
  changeType(type) {
    if (!N.mediaSupports(String(type))) throw new DOMException(`Failed to execute 'changeType' on 'SourceBuffer': The type provided ('${type}') is not supported.`, 'NotSupportedError');
  }
}
defineHandlers(SourceBuffer.prototype, ['updatestart', 'update', 'updateend', 'error', 'abort']);
hidden(G, 'SourceBuffer', SourceBuffer);
class SourceBufferList extends EventTarget {}
hidden(G, 'SourceBufferList', SourceBufferList);

class MediaSource extends EventTarget {
  constructor() {
    super();
    MS.set(this, { id: N.msNew(), sbs: [], readyState: 'closed', el: null });
  }
  static isTypeSupported(t) { return N.mediaSupports(String(t)); }
  static get canConstructInDedicatedWorker() { return false; }
  get readyState() { return MS.get(this).readyState; }
  get duration() { return MS.get(this).readyState === 'closed' ? NaN : N.msDuration(MS.get(this).id); }
  set duration(v) {
    const m = MS.get(this);
    if (m.readyState !== 'open') throw new DOMException("Failed to set the 'duration' property on 'MediaSource': The MediaSource's readyState is not 'open'.", 'InvalidStateError');
    N.msDuration(m.id, Number(v));
    if (m.el) later(() => fireTrusted(m.el, new Event('durationchange')));
  }
  get sourceBuffers() {
    const list = Object.assign(new SourceBufferList(), MS.get(this).sbs);
    Object.defineProperty(list, 'length', { value: MS.get(this).sbs.length });
    return list;
  }
  get activeSourceBuffers() { return this.sourceBuffers; }
  addSourceBuffer(type) {
    const m = MS.get(this);
    if (m.readyState !== 'open') throw new DOMException("Failed to execute 'addSourceBuffer' on 'MediaSource': The MediaSource's readyState is not 'open'.", 'InvalidStateError');
    const r = N.sbNew(m.id, String(type));
    if (typeof r === 'string') throw new DOMException(r, 'NotSupportedError');
    const sb = Object.create(SourceBuffer.prototype);
    SB.set(sb, { id: r, ms: this, updating: false, mode: 'segments', offset: 0, winStart: 0, winEnd: Infinity, removed: false });
    m.sbs.push(sb);
    return sb;
  }
  removeSourceBuffer(sb) {
    const m = MS.get(this);
    const i = m.sbs.indexOf(sb);
    if (i < 0) throw new DOMException("Failed to execute 'removeSourceBuffer' on 'MediaSource': The SourceBuffer provided is not contained in this MediaSource.", 'NotFoundError');
    m.sbs.splice(i, 1);
    SB.get(sb).removed = true;
  }
  endOfStream(error) {
    const m = MS.get(this);
    if (m.readyState !== 'open') throw new DOMException("Failed to execute 'endOfStream' on 'MediaSource': The MediaSource's readyState is not 'open'.", 'InvalidStateError');
    m.readyState = 'ended';
    if (!error) N.msEnd(m.id);
    later(() => fireTrusted(this, new Event('sourceended')));
  }
  setLiveSeekableRange() {}
  clearLiveSeekableRange() {}
}
defineHandlers(MediaSource.prototype, ['sourceopen', 'sourceended', 'sourceclose']);
hidden(G, 'MediaSource', MediaSource);
hidden(G, 'WebKitMediaSource', MediaSource);

const MEDIA_EL = new WeakMap();
const mediaOf = (el) => {
  let s = MEDIA_EL.get(el);
  if (!s) MEDIA_EL.set(el, s = { ms: null, rate: 1, defaultRate: 1, volume: 1, muted: el.hasAttribute('muted'), error: null, network: 0, frames: 0 });
  return s;
};
const stateOf = (el) => N.mediaState(el) || [0, 0, true, false, false, 0, 0, NaN];
function fireMedia(el, types) {
  for (const t of types) later(() => fireTrusted(el, new Event(t)));
}
/// Liga o elemento a uma fonte (blob: de um MediaSource) ou recusa o endereço.
function loadMedia(el, url) {
  const s = mediaOf(el);
  if (s.ms) {
    const old = MS.get(s.ms);
    old.readyState = 'closed';
    old.el = null;
    const prev = s.ms;
    later(() => fireTrusted(prev, new Event('sourceclose')));
    N.mediaDetach(el);
    s.ms = null;
  }
  s.error = null;
  if (!url) { s.network = 0; return; }
  const id = String(url).startsWith('blob:') ? String(url).split('/').pop() : null;
  const src = id ? BLOBS.get(id) : null;
  if (src instanceof MediaSource) {
    s.ms = src;
    s.network = 2;
    const m = MS.get(src);
    m.el = el;
    N.mediaAttach(el, m.id);
    later(() => {
      fireTrusted(el, new Event('loadstart'));
      m.readyState = 'open';
      fireTrusted(src, new Event('sourceopen'));
    });
    return;
  }
  // arquivo comum: o Ferro baixa inteiro e toca (MP4 com H.264/AV1/AAC)
  s.network = 2;
  N.mediaLoadUrl(el, resolveURL(String(url)));
  later(() => fireTrusted(el, new Event('loadstart')));
}
accessor(MEDIA, 'src', function () {
  const v = this.getAttribute('src');
  return v === null ? '' : (v.startsWith('blob:') ? v : resolveURL(v));
}, function (v) {
  v = String(v);
  this.setAttribute('src', v);
  loadMedia(this, v);
});
accessor(MEDIA, 'srcObject', function () { return mediaOf(this).ms; }, function (v) {
  if (v instanceof MediaSource) {
    const u = URL.createObjectURL(v);
    loadMedia(this, u);
  } else loadMedia(this, '');
});
const MEDIA_SET = (el) => { const s = mediaOf(el); N.mediaSet(el, s.rate, s.volume, s.muted); };
accessor(MEDIA, 'currentTime', function () { return stateOf(this)[0]; }, function (t) {
  t = Number(t);
  if (!Number.isFinite(t)) throw new TypeError("Failed to set the 'currentTime' property on 'HTMLMediaElement': The provided double value is non-finite.");
  fireMedia(this, N.mediaSeek(this, t));
});
hidden(MEDIA, 'fastSeek', function (t) { this.currentTime = t; });
accessor(MEDIA, 'duration', function () {
  const s = mediaOf(this);
  return s.ms ? s.ms.duration : NaN;
});
accessor(MEDIA, 'paused', function () { return stateOf(this)[2]; });
accessor(MEDIA, 'seeking', function () { return stateOf(this)[3]; });
accessor(MEDIA, 'ended', function () { return stateOf(this)[4]; });
accessor(MEDIA, 'readyState', function () { return stateOf(this)[1]; });
accessor(MEDIA, 'networkState', function () { return mediaOf(this).network; });
accessor(MEDIA, 'error', function () { return mediaOf(this).error; });
accessor(MEDIA, 'currentSrc', function () { return this.src; });
accessor(MEDIA, 'buffered', function () {
  const s = mediaOf(this);
  return new TimeRanges(s.ms ? N.msBuffered(MS.get(s.ms).id) : []);
});
accessor(MEDIA, 'seekable', function () {
  const d = this.duration;
  return new TimeRanges(Number.isFinite(d) ? [0, d] : []);
});
accessor(MEDIA, 'played', function () { return new TimeRanges([0, this.currentTime]); });
accessor(MEDIA, 'playbackRate', function () { return mediaOf(this).rate; }, function (v) {
  mediaOf(this).rate = Number(v) || 1; MEDIA_SET(this); fireMedia(this, ['ratechange']);
});
accessor(MEDIA, 'defaultPlaybackRate', function () { return mediaOf(this).defaultRate; }, function (v) { mediaOf(this).defaultRate = Number(v) || 1; });
accessor(MEDIA, 'volume', function () { return mediaOf(this).volume; }, function (v) {
  v = Number(v);
  if (!(v >= 0 && v <= 1)) throw new DOMException(`Failed to set the 'volume' property on 'HTMLMediaElement': The volume provided (${v}) is outside the range [0, 1].`, 'IndexSizeError');
  mediaOf(this).volume = v; MEDIA_SET(this); fireMedia(this, ['volumechange']);
});
accessor(MEDIA, 'muted', function () { return mediaOf(this).muted; }, function (v) {
  mediaOf(this).muted = !!v; MEDIA_SET(this); fireMedia(this, ['volumechange']);
});
accessor(MEDIA, 'preservesPitch', () => true, () => {});
accessor(MEDIA, 'textTracks', function () { return Object.assign([], { addEventListener() {}, removeEventListener() {}, getTrackById() { return null; }, onaddtrack: null, onchange: null }); });
accessor(MEDIA, 'audioTracks', function () { return Object.assign([], { addEventListener() {}, removeEventListener() {} }); });
accessor(MEDIA, 'videoTracks', function () { return Object.assign([], { addEventListener() {}, removeEventListener() {} }); });
accessor(MEDIA, 'mediaKeys', () => null);
accessor(MEDIA, 'sinkId', () => '');
accessor(MEDIA, 'remote', () => ({ state: 'disconnected', watchAvailability() { return Promise.reject(new DOMException('', 'NotSupportedError')); }, addEventListener() {} }));
methods(MEDIA, {
  play() {
    const s = mediaOf(this);
    if (s.error) return Promise.reject(new DOMException('The element has no supported sources.', 'NotSupportedError'));
    fireMedia(this, N.mediaPlay(this));
    return Promise.resolve();
  },
  pause() { fireMedia(this, N.mediaPause(this)); },
  load() { loadMedia(this, this.getAttribute('src')); },
  canPlayType(t) { return N.mediaSupports(String(t)) ? 'probably' : ''; },
  addTextTrack() { return { mode: 'disabled', cues: [], addCue() {}, removeCue() {}, addEventListener() {} }; },
  setMediaKeys() { return Promise.reject(new DOMException('', 'NotSupportedError')); },
  setSinkId() { return Promise.resolve(); },
  captureStream() { return null; },
});
const VIDEO = HTMLVideoElement.prototype;
accessor(VIDEO, 'videoWidth', function () { return stateOf(this)[5]; });
accessor(VIDEO, 'videoHeight', function () { return stateOf(this)[6]; });
methods(VIDEO, {
  getVideoPlaybackQuality() {
    return { creationTime: N.now(), totalVideoFrames: stateOf(this)[8] || 0, droppedVideoFrames: 0, corruptedVideoFrames: 0, totalFrameDelay: 0 };
  },
  requestVideoFrameCallback(cb) {
    return requestAnimationFrame((t) => cb(t, { mediaTime: this.currentTime, presentedFrames: 0, width: this.videoWidth, height: this.videoHeight }));
  },
  cancelVideoFrameCallback(id) { cancelAnimationFrame(id); },
  requestPictureInPicture() { return Promise.reject(new DOMException('', 'NotSupportedError')); },
});
accessor(VIDEO, 'disablePictureInPicture', () => true, () => {});
hidden(G, 'Audio', named(function Audio(src) {
  const a = document.createElement('audio');
  if (src !== undefined) a.src = src;
  return a;
}, 'Audio'));

// ------------------------------------------------- mais do Element/Node
const EP = Element.prototype;
methods(EP, {
  getAttributeNS(ns, name) { return this.getAttribute(name); },
  setAttributeNS(ns, name, v) { this.setAttribute(String(name).split(':').pop(), v); },
  removeAttributeNS(ns, name) { this.removeAttribute(name); },
  hasAttributeNS(ns, name) { return this.hasAttribute(name); },
  getElementsByTagNameNS(ns, name) { return this.getElementsByTagName(name); },
  animate() {
    const a = new EventTarget();
    Object.assign(a, { playState: 'finished', finished: Promise.resolve(a), ready: Promise.resolve(a),
      cancel() {}, finish() {}, play() {}, pause() {}, reverse() {}, onfinish: null, oncancel: null, currentTime: 0 });
    setTimeout(() => { if (typeof a.onfinish === 'function') a.onfinish(); }, 0);
    return a;
  },
  getAnimations() { return []; },
  requestFullscreen() {
    if (!this.isConnected) return Promise.reject(new TypeError('Failed to execute \'requestFullscreen\' on \'Element\': Element is not connected.'));
    const prev = N.fullscreenElement();
    N.requestFullscreen(this);
    return new Promise((res) => setTimeout(() => {
      if (prev && prev !== this) fireTrusted(prev, new Event('fullscreenchange', { bubbles: true, composed: true }));
      fireTrusted(this, new Event('fullscreenchange', { bubbles: true, composed: true }));
      fireTrusted(this, new Event('webkitfullscreenchange', { bubbles: true, composed: true }));
      res();
    }, 0));
  },
  webkitRequestFullscreen() { return this.requestFullscreen(); },
  webkitRequestFullScreen() { return this.requestFullscreen(); },
  requestPointerLock() {},
  setPointerCapture() {}, releasePointerCapture() {}, hasPointerCapture() { return false; },
  checkVisibility() { return this.getClientRects().length > 0; },
  computedStyleMap() { return new Map(); },
});
// sem attachShadow nativo: bibliotecas como o ShadyDOM (polyfill) montam o
// shadow DOM dentro do DOM normal, que o Ferro desenha
accessor(EP, 'prefix', () => null);
methods(Node.prototype, {
  lookupNamespaceURI() { return null; },
  lookupPrefix() { return null; },
  isDefaultNamespace() { return false; },
});

// ---------------------------------------------------------- CSS.escape
const CSSNS = {
  escape(v) {
    const s = String(v);
    let out = '';
    for (let i = 0; i < s.length; i++) {
      const c = s.charCodeAt(i);
      const ch = s[i];
      if (c === 0) out += '�';
      else if ((c >= 1 && c <= 0x1f) || c === 0x7f || (i === 0 && c >= 48 && c <= 57) ||
        (i === 1 && c >= 48 && c <= 57 && s.charCodeAt(0) === 45)) out += '\\' + c.toString(16) + ' ';
      else if (i === 0 && s.length === 1 && c === 45) out += '\\' + ch;
      else if (c >= 0x80 || c === 45 || c === 95 || (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122)) out += ch;
      else out += '\\' + ch;
    }
    return out;
  },
  supports() { return true; },
  registerProperty() {},
};
hidden(G, 'CSS', CSSNS);

// ---------------------------------------------------- folhas (CSSOM)
const SHEETS = new WeakMap();
class CSSStyleSheet {
  constructor() { SHEETS.set(this, { rules: [], owner: null }); }
  get cssRules() { return SHEETS.get(this).rules.map((t) => ({ cssText: t, type: 1 })); }
  get rules() { return this.cssRules; }
  get ownerNode() { return SHEETS.get(this).owner; }
  get media() { return { mediaText: '', length: 0 }; }
  get disabled() { return false; }
  insertRule(rule, index = 0) {
    const s = SHEETS.get(this);
    index = Math.min(Math.max(0, index | 0), s.rules.length);
    s.rules.splice(index, 0, String(rule));
    flushSheet(this);
    return index;
  }
  deleteRule(index) {
    const s = SHEETS.get(this);
    s.rules.splice(index | 0, 1);
    flushSheet(this);
  }
  addRule(sel, style, index) { return this.insertRule(`${sel} { ${style} }`, index ?? this.cssRules.length); }
  removeRule(i) { this.deleteRule(i); }
  replaceSync(text) { SHEETS.get(this).rules = [String(text)]; flushSheet(this); }
  replace(text) { this.replaceSync(text); return Promise.resolve(this); }
}
function flushSheet(sheet) {
  const s = SHEETS.get(sheet);
  if (s.owner && s.owner.localName === 'style') {
    s.flushing = true;
    s.owner.textContent = s.base + '\n' + s.rules.join('\n');
    s.flushing = false;
  }
}
function sheetOf(el) {
  let sh = SHEET_OF.get(el);
  if (!sh) {
    sh = new CSSStyleSheet();
    const s = SHEETS.get(sh);
    s.owner = el;
    s.base = el.localName === 'style' ? el.textContent : '';
    SHEET_OF.set(el, sh);
  }
  return sh;
}
const SHEET_OF = new WeakMap();
accessor(HTMLStyleElement.prototype, 'sheet', function () { return this.isConnected ? sheetOf(this) : null; });
hidden(G, 'CSSStyleSheet', CSSStyleSheet);
accessor(Document.prototype, 'styleSheets', function () {
  return [...this.querySelectorAll('style, link[rel~=stylesheet]')].map((e) => e.sheet).filter(Boolean);
});
// adoptedStyleSheets: as folhas viram um <style> dentro da raiz (ou do <head>)
const ADOPTED = new WeakMap();
function adoptedAccessor(P, place) {
  accessor(P, 'adoptedStyleSheets', function () { return (ADOPTED.get(this) || {}).list || []; }, function (list) {
    let st = ADOPTED.get(this);
    if (!st) {
      st = { list: [], el: document.createElement('style') };
      st.el.setAttribute('data-ferro-adopted', '');
      place(this).appendChild(st.el);
      ADOPTED.set(this, st);
    }
    st.list = [...list];
    st.el.textContent = st.list.map((sh) => SHEETS.get(sh) ? SHEETS.get(sh).rules.join('\n') : '').join('\n');
  });
}
adoptedAccessor(Document.prototype, (d) => d.head || d.documentElement);
adoptedAccessor(ShadowRoot.prototype, (r) => r);

// ---------------------------------------------------- Document (JS)
const DP = Document.prototype;
accessor(DP, 'cookie', () => N.cookie(), (v) => N.setCookie(String(v)));
accessor(DP, 'referrer', () => '');
accessor(DP, 'domain', () => N.urlParse(N.href())?.hostname ?? '', () => {});
accessor(DP, 'characterSet', () => 'UTF-8');
accessor(DP, 'charset', () => 'UTF-8');
accessor(DP, 'inputEncoding', () => 'UTF-8');
accessor(DP, 'contentType', () => 'text/html');
accessor(DP, 'compatMode', function () { return this.doctype ? 'CSS1Compat' : 'BackCompat'; });
accessor(DP, 'visibilityState', () => 'visible');
accessor(DP, 'hidden', () => false);
accessor(DP, 'fullscreenElement', () => N.fullscreenElement());
accessor(DP, 'webkitFullscreenElement', () => N.fullscreenElement());
accessor(DP, 'webkitCurrentFullScreenElement', () => N.fullscreenElement());
accessor(DP, 'fullscreen', () => !!N.fullscreenElement());
accessor(DP, 'webkitIsFullScreen', () => !!N.fullscreenElement());
accessor(DP, 'fullscreenEnabled', () => true);
accessor(DP, 'webkitFullscreenEnabled', () => true);
accessor(DP, 'pointerLockElement', () => null);
accessor(DP, 'scrollingElement', function () { return this.documentElement; });
accessor(DP, 'lastModified', () => new Date().toLocaleString('en-US'));
accessor(DP, 'designMode', () => 'off', () => {});
accessor(DP, 'dir', function () { return this.documentElement?.dir ?? ''; }, function (v) { if (this.documentElement) this.documentElement.dir = v; });
accessor(DP, 'location', () => G.location, (v) => { G.location.href = v; });
accessor(DP, 'all', function () { return this.getElementsByTagName('*'); });
accessor(DP, 'embeds', function () { return this.getElementsByTagName('embed'); });
accessor(DP, 'plugins', function () { return this.getElementsByTagName('embed'); });
const FONTS = Object.assign(new EventTarget(), {
  ready: Promise.resolve(), status: 'loaded', size: 0,
  check() { return true; }, load() { return Promise.resolve([]); }, add() {}, delete() {}, clear() {},
  forEach() {}, has() { return false; }, values() { return [].values(); }, [Symbol.iterator]() { return [].values(); },
});
FONTS.ready = Promise.resolve(FONTS);
accessor(DP, 'fonts', () => FONTS);
const EVENT_CTORS = {
  event: 'Event', events: 'Event', htmlevents: 'Event', customevent: 'CustomEvent', uievent: 'UIEvent',
  uievents: 'UIEvent', mouseevent: 'MouseEvent', mouseevents: 'MouseEvent', keyboardevent: 'KeyboardEvent',
  keyboardevents: 'KeyboardEvent', focusevent: 'FocusEvent', messageevent: 'MessageEvent',
  hashchangeevent: 'HashChangeEvent', popstateevent: 'PopStateEvent', errorevent: 'ErrorEvent',
  progressevent: 'ProgressEvent', pointerevent: 'PointerEvent', wheelevent: 'WheelEvent', inputevent: 'InputEvent',
  storageevent: 'StorageEvent', svgevents: 'Event', touchevent: null,
};
methods(DP, {
  createEvent(kind) {
    const c = EVENT_CTORS[String(kind).toLowerCase()];
    if (!c) throw new DOMException(`Failed to execute 'createEvent' on 'Document': The provided event type ('${kind}') is invalid.`, 'NotSupportedError');
    return new G[c]('');
  },
  createRange() { return new Range(); },
  getSelection() { return SELECTION; },
  execCommand() { return false; },
  queryCommandSupported() { return false; },
  queryCommandEnabled() { return false; },
  queryCommandState() { return false; },
  queryCommandValue() { return ''; },
  createTreeWalker(root, what = 0xFFFFFFFF, filter = null) { return new TreeWalker(root, what, filter); },
  createNodeIterator(root, what = 0xFFFFFFFF, filter = null) { return new NodeIterator(root, what, filter); },
  createAttribute(name) { return { name: String(name).toLowerCase(), value: '', specified: true }; },
  exitFullscreen() {
    const el = N.exitFullscreen();
    if (!el) return Promise.reject(new TypeError('Document not active'));
    return new Promise((res) => setTimeout(() => { hooks.fullscreenChanged(el); res(); }, 0));
  },
  webkitExitFullscreen() { return this.exitFullscreen(); },
  webkitCancelFullScreen() { return this.exitFullscreen(); },
  exitPointerLock() {},
  startViewTransition(cb) {
    const done = Promise.resolve().then(() => cb && cb());
    return { finished: done, ready: done, updateCallbackDone: done, skipTransition() {} };
  },
  hasStorageAccess() { return Promise.resolve(true); },
  requestStorageAccess() { return Promise.resolve(); },
});
hidden(G, 'getSelection', () => SELECTION);
const SELECTION = {
  rangeCount: 0, isCollapsed: true, type: 'None', anchorNode: null, focusNode: null, anchorOffset: 0, focusOffset: 0,
  removeAllRanges() {}, addRange() {}, getRangeAt() { return new Range(); }, collapse() {}, empty() {},
  selectAllChildren() {}, toString() { return ''; }, containsNode() { return false; }, extend() {}, modify() {},
};

// Range: o bastante para createContextualFragment e medidas simples
class Range {
  constructor() { this.startContainer = document; this.endContainer = document; this.startOffset = 0; this.endOffset = 0; this.collapsed = true; }
  setStart(n, o) { this.startContainer = n; this.startOffset = o; }
  setEnd(n, o) { this.endContainer = n; this.endOffset = o; this.collapsed = false; }
  setStartBefore(n) { this.startContainer = n; } setStartAfter(n) { this.startContainer = n; }
  setEndBefore(n) { this.endContainer = n; } setEndAfter(n) { this.endContainer = n; }
  selectNode(n) { this.startContainer = this.endContainer = n; }
  selectNodeContents(n) { this.startContainer = this.endContainer = n; this.endOffset = n.childNodes.length; }
  collapse() { this.collapsed = true; }
  get commonAncestorContainer() { return this.startContainer; }
  cloneRange() { return Object.assign(new Range(), this); }
  detach() {}
  createContextualFragment(html) {
    const t = document.createElement('template');
    t.innerHTML = html;
    return t.content;
  }
  getBoundingClientRect() {
    const n = this.startContainer;
    return n && n.getBoundingClientRect ? n.getBoundingClientRect() : new DOMRect();
  }
  getClientRects() { return [this.getBoundingClientRect()]; }
  toString() { return this.startContainer ? this.startContainer.textContent || '' : ''; }
  deleteContents() {} insertNode(n) { this.startContainer.appendChild(n); }
  surroundContents(n) { this.startContainer.appendChild(n); }
  extractContents() { return document.createDocumentFragment(); }
  cloneContents() { return document.createDocumentFragment(); }
  compareBoundaryPoints() { return 0; }
  intersectsNode() { return false; }
}
Range.START_TO_START = 0; Range.START_TO_END = 1; Range.END_TO_END = 2; Range.END_TO_START = 3;
hidden(G, 'Range', Range);
hidden(G, 'StaticRange', Range);

// TreeWalker / NodeIterator
const NodeFilter = {
  FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3, SHOW_ALL: 0xFFFFFFFF, SHOW_ELEMENT: 1, SHOW_ATTRIBUTE: 2,
  SHOW_TEXT: 4, SHOW_CDATA_SECTION: 8, SHOW_PROCESSING_INSTRUCTION: 64, SHOW_COMMENT: 128, SHOW_DOCUMENT: 256,
  SHOW_DOCUMENT_TYPE: 512, SHOW_DOCUMENT_FRAGMENT: 1024,
};
hidden(G, 'NodeFilter', NodeFilter);
function acceptNode(w, n) {
  if (!(w.whatToShow & (1 << (n.nodeType - 1)))) return 3;
  const f = w.filter;
  if (!f) return 1;
  return typeof f === 'function' ? f(n) : f.acceptNode(n);
}
function nextInTree(n, root) {
  if (n.firstChild) return n.firstChild;
  while (n && n !== root) {
    if (n.nextSibling) return n.nextSibling;
    n = n.parentNode;
  }
  return null;
}
function prevInTree(n, root) {
  if (n === root) return null;
  let p = n.previousSibling;
  if (!p) return n.parentNode;
  while (p.lastChild) p = p.lastChild;
  return p;
}
class TreeWalker {
  constructor(root, whatToShow, filter) { this.root = root; this.whatToShow = whatToShow; this.filter = filter; this.currentNode = root; }
  nextNode() {
    let n = this.currentNode;
    while ((n = nextInTree(n, this.root))) {
      const r = acceptNode(this, n);
      if (r === 1) return (this.currentNode = n);
      if (r === 2) {
        // pula a subárvore
        let m = n;
        while (m && m !== this.root && !m.nextSibling) m = m.parentNode;
        if (!m || m === this.root) return null;
        n = m.nextSibling;
        if (acceptNode(this, n) === 1) return (this.currentNode = n);
        this.currentNode = n;
      }
    }
    return null;
  }
  previousNode() {
    let n = this.currentNode;
    while ((n = prevInTree(n, this.root))) {
      if (acceptNode(this, n) === 1) return (this.currentNode = n);
      if (n === this.root) return null;
    }
    return null;
  }
  parentNode() {
    let n = this.currentNode;
    while (n && n !== this.root) {
      n = n.parentNode;
      if (n && acceptNode(this, n) === 1) return (this.currentNode = n);
    }
    return null;
  }
  firstChild() {
    for (let c = this.currentNode.firstChild; c; c = c.nextSibling) if (acceptNode(this, c) === 1) return (this.currentNode = c);
    return null;
  }
  lastChild() {
    for (let c = this.currentNode.lastChild; c; c = c.previousSibling) if (acceptNode(this, c) === 1) return (this.currentNode = c);
    return null;
  }
  nextSibling() {
    for (let c = this.currentNode.nextSibling; c; c = c.nextSibling) if (acceptNode(this, c) === 1) return (this.currentNode = c);
    return null;
  }
  previousSibling() {
    for (let c = this.currentNode.previousSibling; c; c = c.previousSibling) if (acceptNode(this, c) === 1) return (this.currentNode = c);
    return null;
  }
}
class NodeIterator extends TreeWalker {
  constructor(root, w, f) { super(root, w, f); this.referenceNode = root; this.started = false; }
  nextNode() {
    if (!this.started) { this.started = true; if (acceptNode(this, this.root) === 1) return this.root; }
    let n = this.currentNode;
    while ((n = nextInTree(n, this.root))) {
      this.currentNode = n;
      if (acceptNode(this, n) === 1) return n;
    }
    return null;
  }
  detach() {}
}
hidden(G, 'TreeWalker', TreeWalker);
hidden(G, 'NodeIterator', NodeIterator);

// ------------------------------------------------------- location
class Location {
  constructor() { throw new TypeError('Illegal constructor'); }
}
const LOC = Object.create(Location.prototype);
function hashChange(oldURL) {
  const newURL = N.href();
  if (oldURL === newURL) return;
  setTimeout(() => {
    fireTrusted(G, new G.HashChangeEvent('hashchange', { oldURL, newURL }));
  }, 0);
}
function go(url) {
  const old = N.href();
  if (!N.navigate(String(url))) hashChange(old);
}
for (const part of ['protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin']) {
  accessor(Location.prototype, part, () => N.urlParse(N.href())[part], part === 'origin' ? undefined : (v) => {
    let val = String(v);
    if (part === 'hash' && val && !val.startsWith('#')) val = '#' + val;
    go(N.urlSet(N.href(), part, val));
  });
}
accessor(Location.prototype, 'href', () => N.href(), (v) => go(v));
methods(Location.prototype, {
  assign(u) { go(u); },
  replace(u) { go(u); },
  reload() { N.reload(); },
  toString() { return N.href(); },
  valueOf() { return this; },
});
accessor(Location.prototype, 'ancestorOrigins', () => []);
hidden(G, 'Location', Location);
Object.defineProperty(G, 'location', { get: () => LOC, set: (v) => go(v), configurable: false, enumerable: true });

// ------------------------------------------------------- history
let historyState = null;
let historyLength = 1;
class History {
  constructor() { throw new TypeError('Illegal constructor'); }
}
methods(History.prototype, {
  pushState(state, title, url) {
    historyState = state === undefined ? null : structuredClone(state);
    historyLength++;
    if (url !== undefined && url !== null) N.setUrl(String(url));
  },
  replaceState(state, title, url) {
    historyState = state === undefined ? null : structuredClone(state);
    if (url !== undefined && url !== null) N.setUrl(String(url));
  },
  back() {}, forward() {}, go() {},
});
accessor(History.prototype, 'state', () => historyState);
accessor(History.prototype, 'length', () => historyLength);
accessor(History.prototype, 'scrollRestoration', () => 'auto', () => {});
hidden(G, 'History', History);
hidden(G, 'history', Object.create(History.prototype));

// ---------------------------------------------------- navigator, screen
const NAV = {
  userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) Ferro/0.1',
  appVersion: '5.0 (Windows NT 10.0; Win64; x64) Ferro/0.1',
  appName: 'Netscape', appCodeName: 'Mozilla', product: 'Gecko', productSub: '20030107', vendor: '', vendorSub: '',
  platform: 'Win32', language: 'pt-BR', languages: Object.freeze(['pt-BR', 'pt', 'en-US', 'en']),
  cookieEnabled: true, onLine: true, hardwareConcurrency: 8, deviceMemory: 8, maxTouchPoints: 0,
  doNotTrack: null, webdriver: false, pdfViewerEnabled: false, plugins: [], mimeTypes: [],
  javaEnabled() { return false; },
  sendBeacon(url, data) {
    try { G.fetch(url, { method: 'POST', body: data, keepalive: true }).catch(() => {}); } catch (e) {}
    return true;
  },
  vibrate() { return false; },
  registerProtocolHandler() {},
  clipboard: { writeText() { return Promise.resolve(); }, readText() { return Promise.resolve(''); }, write() { return Promise.resolve(); }, read() { return Promise.resolve([]); } },
  permissions: { query() { return Promise.resolve({ state: 'prompt', onchange: null, addEventListener() {} }); } },
  storage: { estimate() { return Promise.resolve({ quota: 1 << 30, usage: 0 }); }, persist() { return Promise.resolve(false); }, persisted() { return Promise.resolve(false); } },
  mediaSession: { metadata: null, playbackState: 'none', setActionHandler() {}, setPositionState() {} },
  getGamepads() { return []; },
  locks: { request(name, opts, cb) { cb = cb || opts; return Promise.resolve().then(() => cb({ name })); }, query() { return Promise.resolve({ held: [], pending: [] }); } },
};
class Navigator {
  constructor() { throw new TypeError('Illegal constructor'); }
}
hidden(G, 'Navigator', Navigator);
hidden(G, 'navigator', Object.setPrototypeOf(NAV, Navigator.prototype));
hidden(G, 'clientInformation', NAV);
hidden(G, 'screen', {
  width: 1920, height: 1080, availWidth: 1920, availHeight: 1040, availLeft: 0, availTop: 0,
  colorDepth: 24, pixelDepth: 24, isExtended: false,
  orientation: Object.assign(new EventTarget(), { type: 'landscape-primary', angle: 0, onchange: null, lock() { return Promise.reject(new DOMException('', 'NotSupportedError')); }, unlock() {} }),
});
hidden(G, 'visualViewport', Object.assign(new EventTarget(), {
  get width() { return G.innerWidth; }, get height() { return G.innerHeight; }, scale: 1, offsetLeft: 0, offsetTop: 0,
  get pageLeft() { return 0; }, get pageTop() { return G.scrollY; }, onresize: null, onscroll: null,
}));
methods(G, {
  scrollTo(x, y) {
    const top = typeof x === 'object' && x ? x.top : y;
    if (top !== undefined) N.scrollTo(Number(top) || 0);
  },
  scroll(x, y) { G.scrollTo(x, y); },
  scrollBy(x, y) {
    const dy = typeof x === 'object' && x ? x.top : y;
    if (dy !== undefined) N.scrollTo(G.scrollY + (Number(dy) || 0));
  },
  open() { return null; },
  close() {},
  stop() {},
  moveTo() {}, moveBy() {}, resizeTo() {}, resizeBy() {},
  postMessage(data, origin) {
    const ev = new MessageEvent('message', { data: structuredClone(data), origin: G.origin, source: G });
    setTimeout(() => fireTrusted(G, ev), 0);
  },
  requestIdleCallback(cb) {
    return setTimeout(() => cb({ didTimeout: false, timeRemaining: () => 50 }), 1);
  },
  cancelIdleCallback(id) { clearTimeout(id); },
  matchMedia(q) { return new MediaQueryList(String(q)); },
  getScreenDetails() { return Promise.reject(new DOMException('', 'NotSupportedError')); },
});

class MediaQueryList extends EventTarget {
  constructor(q) { super(); this.media = q; this.onchange = null; }
  get matches() { return N.matchMedia(this.media); }
  addListener(cb) { this.addEventListener('change', cb); }
  removeListener(cb) { this.removeEventListener('change', cb); }
}
hidden(G, 'MediaQueryList', MediaQueryList);

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
  timing: { navigationStart: T0, fetchStart: T0, domainLookupStart: T0, domainLookupEnd: T0, connectStart: T0,
    connectEnd: T0, requestStart: T0, responseStart: T0, responseEnd: T0, domLoading: T0, domInteractive: T0,
    domContentLoadedEventStart: T0, domContentLoadedEventEnd: T0, domComplete: T0, loadEventStart: T0, loadEventEnd: T0 },
  navigation: { type: 0, redirectCount: 0 },
});
hidden(G, 'performance', PERF);
class PerformanceObserver {
  constructor(cb) { this.cb = cb; }
  observe() {} disconnect() {} takeRecords() { return []; }
  static get supportedEntryTypes() { return ['mark', 'measure']; }
}
hidden(G, 'PerformanceObserver', PerformanceObserver);

// ---------------------------------------------------- armazenamento
class Storage {
  constructor() { throw new TypeError('Illegal constructor'); }
}
function makeStorage(session) {
  const api = Object.create(Storage.prototype);
  const proto = {
    getItem: (k) => N.storageGet(session, String(k)),
    setItem: (k, v) => N.storageSet(session, String(k), String(v)),
    removeItem: (k) => N.storageRemove(session, String(k)),
    clear: () => N.storageClear(session),
    key: (i) => N.storageKeys(session)[i] ?? null,
  };
  return new Proxy(api, {
    get(t, k) {
      if (k === 'length') return N.storageKeys(session).length;
      if (typeof k === 'symbol') return t[k];
      if (Object.prototype.hasOwnProperty.call(proto, k)) return proto[k];
      if (k in Storage.prototype || k in Object.prototype) return t[k];
      const v = N.storageGet(session, k);
      return v === null ? undefined : v;
    },
    set(t, k, v) { if (typeof k === 'string') N.storageSet(session, k, String(v)); return true; },
    has(t, k) { return typeof k === 'string' && (k in proto || N.storageGet(session, k) !== null); },
    deleteProperty(t, k) { N.storageRemove(session, String(k)); return true; },
    ownKeys() { return N.storageKeys(session); },
    getOwnPropertyDescriptor(t, k) {
      const v = typeof k === 'string' ? N.storageGet(session, k) : null;
      return v === null ? undefined : { value: v, writable: true, enumerable: true, configurable: true };
    },
  });
}
hidden(G, 'Storage', Storage);
const LOCAL = makeStorage(false);
const SESSION = makeStorage(true);
Object.defineProperty(G, 'localStorage', { get: () => LOCAL, configurable: true, enumerable: true });
Object.defineProperty(G, 'sessionStorage', { get: () => SESSION, configurable: true, enumerable: true });

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
  if (v instanceof Node) throw new DOMException(`${Object.prototype.toString.call(v)} could not be cloned.`, 'DataCloneError');
  out = {};
  seen.set(v, out);
  for (const k of Object.keys(v)) out[k] = clone(v[k], seen);
  return out;
}
hidden(G, 'structuredClone', function structuredClone(v) { return clone(v, new Map()); });

// ------------------------------------------------- crypto
let seed = (Date.now() ^ (N.now() * 1e6)) >>> 0 || 0x9e3779b9;
function rand32() {
  // xorshift32 misturado com Math.random (não é criptográfico!)
  seed ^= seed << 13; seed >>>= 0; seed ^= seed >>> 17; seed ^= seed << 5; seed >>>= 0;
  return (seed ^ Math.floor(Math.random() * 0x100000000)) >>> 0;
}
hidden(G, 'crypto', {
  getRandomValues(arr) {
    if (!ArrayBuffer.isView(arr)) throw new TypeError("Failed to execute 'getRandomValues' on 'Crypto': parameter 1 is not of type 'ArrayBufferView'.");
    const bytes = new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength);
    for (let i = 0; i < bytes.length; i++) bytes[i] = rand32() & 255;
    return arr;
  },
  randomUUID() {
    const b = G.crypto.getRandomValues(new Uint8Array(16));
    b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80;
    const h = [...b].map((x) => x.toString(16).padStart(2, '0')).join('');
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
  },
  subtle: {},
});

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
    const id = G.crypto.randomUUID();
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
    if (form) for (const [k, v] of formEntries(form, submitter)) this._list.push([k, v]);
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

