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

