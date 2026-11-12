const u8 = new Uint8Array([1, 2, 300, -1, 3.7]);
console.log(u8, u8.length, u8.byteLength, u8[2], u8[10], Uint8Array.BYTES_PER_ELEMENT, Object.prototype.toString.call(u8));
const c = new Uint8ClampedArray([300, -5, 1.5, 2.5, 0.5, 254.5]); console.log(c);
const buf = new ArrayBuffer(8); const v32 = new Uint32Array(buf); const v8 = new Uint8Array(buf, 2, 4);
v32[0] = 0x04030201; v8[0] = 0xff; console.log(buf.byteLength, v32, v8, v8.byteOffset, v8.buffer === buf);
const f = new Float64Array(3); f.set([1.5, 2.25], 1); console.log(f, f.map(x => x * 2), f.filter(x => x > 1), Array.from(f));
const i16 = Int16Array.from([40000, -40000, 7], x => x); console.log(i16, i16.indexOf(7), i16.includes(-25536), i16.join("|"));
const s = new Int32Array([5, -1, 10, -1, 3]); console.log(s.sort(), [...s.subarray(1, 3)], s.slice(-2), s.reverse(), s.at(-1));
const f32 = new Float32Array([0.1, NaN, -0, 0, -Infinity]); console.log(f32, f32.toSorted(), f32[0] === 0.1, Math.fround(0.1) === f32[0]);
const b64 = new BigInt64Array([1n, -1n, 2n ** 63n]); console.log(b64, new BigUint64Array(b64.buffer));
const dv = new DataView(new ArrayBuffer(16)); dv.setUint16(0, 0x1234); dv.setUint16(2, 0x1234, true); dv.setFloat64(8, Math.PI);
console.log(dv.getUint8(0), dv.getUint8(1), dv.getUint8(2), dv.getUint16(2, true), dv.getFloat64(8).toFixed(5), dv.byteLength, dv.getInt8(0));
dv.setBigInt64(0, -2n); console.log(dv.getBigInt64(0), dv.getBigUint64(0), dv.getInt32(4), dv.getUint32(0));
for (const t of [() => new Uint8Array(-1), () => dv.getUint32(14), () => new Int32Array(new ArrayBuffer(7)), () => new Int32Array(new ArrayBuffer(8), 2), () => Uint8Array(2), () => new BigInt64Array([1])]) {
  try { console.log(t()); } catch (e) { console.log(e.name + ": " + e.message); }
}
console.log(ArrayBuffer.isView(u8), ArrayBuffer.isView(dv), ArrayBuffer.isView(buf), buf.slice(0, 2).byteLength, new Uint8Array(buf.slice(4)));
const copia = new Uint8Array(u8); copia[0] = 99; console.log(u8[0], copia[0], new Uint16Array(new Uint8Array([1, 2, 3])), Object.keys(new Uint8Array(3)));
console.log(JSON.stringify(new Uint8Array([1, 2])), [...new Uint8Array([7, 8]).entries()], new Uint8Array(4).fill(9, 1, 3), new Int8Array(2).every(x => x === 0));
const bytes = new TextEncoderFallback().encode("Olá"); console.log(bytes);
function TextEncoderFallback() { this.encode = s => new Uint8Array([...unescape(encodeURIComponent(s))].map(c => c.charCodeAt(0))); }
console.log(new Float64Array([1, 2, 3]).reduce((a, b) => a + b), new Uint8Array([3, 1, 2]).toSorted((a, b) => b - a), new Uint8Array(0));
