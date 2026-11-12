const casos = [
  () => undefined.x, () => null.y.z, () => ({}).f(), () => naoDefinido, () => { const c = 1; c = 2; },
  () => new (class { constructor() { throw new RangeError("dentro"); } })(), () => [].reduce((a, b) => a),
  () => JSON.parse("{bad"), () => new Array(-1), () => (1).toFixed(200), () => Symbol() + "",
  () => { "use strict"; undefinedVar = 1; }, () => Object.defineProperty(Object.freeze({}), "x", { value: 1 }),
  () => { let a = {}; a.b.c = 1; }, () => (void 0)(), () => ({}) instanceof 5, () => "x" in "str",
  () => { class A { constructor() { this.x; } } class B extends A { constructor() { this.y = 1; super(); } } new B(); },
  () => [].at.call(null), () => new Set(5), () => Reflect.ownKeys(1), () => structuredCloneNaoExiste(),
];
for (const c of casos) { try { c(); console.log("sem erro"); } catch (e) { console.log(e.name + ": " + e.message); } }
const e = new Error("msg", { cause: "causa" }); console.log(e.cause, e instanceof Error, Object.keys(e), e.toString());
const t = new TypeError(); console.log(t.message === "", t.name, String(t), Object.prototype.toString.call(t));
try { throw { custom: true }; } catch (x) { console.log(x); }
try { throw "string"; } catch (x) { console.log(typeof x, x); }
const errs = [new RangeError("r"), new SyntaxError("s"), new ReferenceError("f"), new EvalError("e"), new URIError("u")];
console.log(errs.map(e => e instanceof Error && e.constructor.name));
function profundo(n) { return n === 0 ? new Error("fundo").stack.split("\n").length : profundo(n - 1); } console.log(profundo(3) > 3);
console.log(Object.prototype.hasOwnProperty.call(new Error("x"), "message"), Object.prototype.hasOwnProperty.call(new Error(), "message"));
const agg = new AggregateError([new Error("a"), "b"], "várias"); console.log(agg.name, agg.message, agg.errors.length);
