function f(a, b = 2, ...c) { return [a, b, c, arguments.length]; }
console.log(f(1), f(1, undefined, 3, 4), f.length);
const g = function () { return typeof this; };
console.log(g(), g.call(5));
function outer() { const self = this; return [() => this === self, function () { return this; }]; }
const [arrow, plain] = outer.call({ x: 1 });
console.log(arrow(), typeof plain());
function counter() { let c = 0; return { inc: () => ++c, dec: () => --c, get value() { return c; } }; }
const k = counter(); k.inc(); k.inc(); k.dec(); console.log(k.value);
const memo = (fn, cache = new Map()) => n => cache.has(n) ? cache.get(n) : (cache.set(n, fn(n)), cache.get(n));
const sq = memo(x => x * x); console.log(sq(9), sq(9));
console.log([1, 2, 3].map((x, i, arr) => x * i + arr.length));
const compose = (...fns) => x => fns.reduceRight((v, f) => f(v), x);
console.log(compose(x => x + 1, x => x * 2)(5));
function curry(fn) { return function c(...args) { return args.length >= fn.length ? fn(...args) : (...more) => c(...args, ...more); }; }
console.log(curry((a, b, c) => a + b + c)(1)(2)(3));
const bound = function (p, q) { return [this.v, p, q]; }.bind({ v: "v" }, "p");
console.log(bound("q"), bound.name, bound.length);
console.log((function () { return arguments; })(1, "a"));
const obj = { name: "obj", regular() { return this.name; }, nested() { return [1].map(() => this.name); } };
console.log(obj.regular(), obj.nested());
function Ctor() { if (!new.target) return "sem new"; this.ok = true; }
console.log(Ctor(), new Ctor());
console.log(typeof function () {}.prototype, typeof (() => {}).prototype, Object.keys(function () {}.prototype));
var hoist = typeof later; function later() {} console.log(hoist);
console.log((function fact(n) { return n ? n * fact(n - 1) : 1; })(5));
console.log([function namedFn() {}, () => {}].map(f => f.name));
const named = () => {}; const obj2 = { method() {}, ["comp" + "uted"]: function () {}, get acc() { return 1; } };
console.log(named.name, obj2.method.name, obj2.computed.name, Object.getOwnPropertyDescriptor(obj2, "acc").get.name);
console.log((function (a, b) {}).length, ((a, b = 1, c) => {}).length, ((...r) => {}).length);
function vars() { var x = 1; { var x = 2; } return x; } console.log(vars());
function shadow(x) { { let x = "inner"; } return x; } console.log(shadow("param"));
const fns = []; for (var i = 0; i < 3; i++) { fns.push(((j) => () => j)(i)); } console.log(fns.map(f => f()));
console.log(Function.prototype.toString.call(Math.max).includes("native code"), (x => x * 2).toString());
