const a = 12345678901234567890n, b = 98765432109876543210n;
console.log(a + b, a * b, b / a, b % a, a - b, -a, 2n ** 100n, typeof a, 0x1fn, 0b1010n, 0o17n);
console.log(a < b, a > 1e19, 10n == 10, 10n === 10, 10n == "10", 5n < 5.5, [3n, 1n, 2n].sort(), 0n ? "sim" : "não");
console.log(BigInt(42), BigInt("0xff"), BigInt(" 123 "), BigInt(true), BigInt.asIntN(8, 255n), BigInt.asUintN(64, -1n));
console.log((255n).toString(16), (-255n).toString(2), String(10n ** 30n), `${7n}`, 7n + "x", Number(2n ** 64n), parseInt("9007199254740993n"));
console.log(-7n / 2n, -7n % 2n, 7n >> 1n, -9n >> 1n, 1n << 70n, 6n & -3n, 6n | -3n, 6n ^ 3n, ~5n);
let c = 0n; c++; c += 10n; c *= 3n; c--; console.log(c, c > 30, Object(1n), typeof Object(1n));
for (const f of [() => 1n + 1, () => BigInt(1.5), () => BigInt("x"), () => 1n / 0n, () => +1n, () => 1n >>> 0n, () => JSON.stringify({ x: 1n }), () => new BigInt(1), () => Math.max(1n)]) {
  try { console.log(f()); } catch (e) { console.log(e.name + ": " + e.message); }
}
const m = new Map([[1n, "um"]]); console.log(m.get(1n), m.has(1), new Set([1n, 1n, 2n]).size, [1n, 2n].includes(2n), Object.is(0n, -0n));
console.log({ big: 123n, arr: [1n, -2n] }, [10n ** 20n], BigInt.prototype.toString.call(99n), (1234567n).toLocaleString("en-US"));
const fat = n => n <= 1n ? 1n : n * fat(n - 1n); console.log(fat(30n), String(fat(50n)).length);
console.log(2n ** 64n - 1n === BigInt.asUintN(64, -1n), 0n == false, 1n == true, 1n < "2", "10" > 9n, 2n > NaN, 3n == 3.000);
