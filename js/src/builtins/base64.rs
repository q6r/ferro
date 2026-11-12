//! Uint8Array em base64 e hexadecimal: fromBase64, fromHex, toBase64,
//! toHex, setFromBase64 e setFromHex.

use super::Realm;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Vm, truthy};

pub fn install(r: &Realm, ctor: &ObjRef, proto: &ObjRef) {
    r.method(ctor, "fromBase64", 1, from_base64);
    r.method(ctor, "fromHex", 1, from_hex);
    r.method(proto, "toBase64", 0, to_base64);
    r.method(proto, "toHex", 0, to_hex);
    r.method(proto, "setFromBase64", 1, set_from_base64);
    r.method(proto, "setFromHex", 1, set_from_hex);
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Clone, Copy, PartialEq)]
enum LastChunk {
    Loose,
    Strict,
    StopBeforePartial,
}

/// Resultado de uma decodificação: quanto foi lido, os bytes e se houve erro
/// de sintaxe (os bytes até o erro ainda valem para setFrom*).
struct Decoded {
    read: usize,
    bytes: Vec<u8>,
    error: bool,
}

fn arg_string(vm: &mut Vm, a: &CallArgs) -> Result<JsStr, Value> {
    match a.arg(0) {
        Value::String(s) => Ok(s),
        _ => Err(vm.type_error("The argument must be a string")),
    }
}

/// GetOptionsObject: undefined vira objeto vazio; não-objeto é erro.
fn options(vm: &mut Vm, v: &Value) -> Result<Option<ObjRef>, Value> {
    match v {
        Value::Undefined => Ok(None),
        Value::Object(o) => Ok(Some(o.clone())),
        _ => Err(vm.type_error("The options argument must be an object")),
    }
}

fn option(vm: &mut Vm, o: &Option<ObjRef>, name: &str) -> Result<Value, Value> {
    match o {
        Some(o) => vm.get_obj(o, &PropKey::str(name)),
        None => Ok(Value::Undefined),
    }
}

/// Opção `alphabet`: true para base64url.
fn alphabet(vm: &mut Vm, o: &Option<ObjRef>) -> Result<bool, Value> {
    match option(vm, o, "alphabet")? {
        Value::Undefined => Ok(false),
        Value::String(s) if s.eq_str("base64") => Ok(false),
        Value::String(s) if s.eq_str("base64url") => Ok(true),
        _ => Err(vm.type_error("expected alphabet to be either \"base64\" or \"base64url\"")),
    }
}

fn last_chunk(vm: &mut Vm, o: &Option<ObjRef>) -> Result<LastChunk, Value> {
    match option(vm, o, "lastChunkHandling")? {
        Value::Undefined => Ok(LastChunk::Loose),
        Value::String(s) if s.eq_str("loose") => Ok(LastChunk::Loose),
        Value::String(s) if s.eq_str("strict") => Ok(LastChunk::Strict),
        Value::String(s) if s.eq_str("stop-before-partial") => Ok(LastChunk::StopBeforePartial),
        _ => Err(vm.type_error("expected lastChunkHandling to be either \"loose\", \"strict\", or \"stop-before-partial\"")),
    }
}

fn is_ascii_space(c: u16) -> bool {
    matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

fn skip_space(s: &[u16], mut i: usize) -> usize {
    while i < s.len() && is_ascii_space(s[i]) {
        i += 1;
    }
    i
}

fn b64_value(c: u16) -> Option<u32> {
    let c = u8::try_from(c).ok()?;
    ALPHABET.iter().position(|&x| x == c).map(|p| p as u32)
}

/// DecodeBase64Chunk: 2, 3 ou 4 dígitos → 1, 2 ou 3 bytes (None: bits
/// sobrando com `strict`).
fn decode_chunk(chunk: &[u32], strict: bool) -> Option<Vec<u8>> {
    let mut n = 0u32;
    for k in 0..4 {
        n = n << 6 | chunk.get(k).copied().unwrap_or(0);
    }
    let b = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
    match chunk.len() {
        2 => (!strict || b[1] == 0).then(|| vec![b[0]]),
        3 => (!strict || b[2] == 0).then(|| vec![b[0], b[1]]),
        _ => Some(b.to_vec()),
    }
}

/// FromBase64.
fn from_base64_str(s: &[u16], url: bool, last: LastChunk, max: usize) -> Decoded {
    let mut out = Decoded { read: 0, bytes: Vec::new(), error: false };
    if max == 0 {
        return out;
    }
    let fail = |mut d: Decoded| {
        d.error = true;
        d
    };
    let mut chunk: Vec<u32> = Vec::with_capacity(4);
    let mut i = 0;
    let len = s.len();
    loop {
        i = skip_space(s, i);
        if i == len {
            if !chunk.is_empty() {
                match last {
                    LastChunk::StopBeforePartial => return out,
                    LastChunk::Loose if chunk.len() == 1 => return fail(out),
                    LastChunk::Loose => out.bytes.extend(decode_chunk(&chunk, false).unwrap_or_default()),
                    LastChunk::Strict => return fail(out),
                }
            }
            out.read = len;
            return out;
        }
        let mut c = s[i];
        i += 1;
        if c == b'=' as u16 {
            if chunk.len() < 2 {
                return fail(out);
            }
            i = skip_space(s, i);
            if chunk.len() == 2 {
                if i == len {
                    if last == LastChunk::StopBeforePartial {
                        return out;
                    }
                    return fail(out);
                }
                if s[i] == b'=' as u16 {
                    i = skip_space(s, i + 1);
                }
            }
            if i < len {
                return fail(out);
            }
            match decode_chunk(&chunk, last == LastChunk::Strict) {
                Some(b) => out.bytes.extend(b),
                None => return fail(out),
            }
            out.read = len;
            return out;
        }
        if url {
            if c == b'+' as u16 || c == b'/' as u16 {
                return fail(out);
            } else if c == b'-' as u16 {
                c = b'+' as u16;
            } else if c == b'_' as u16 {
                c = b'/' as u16;
            }
        }
        let Some(v) = b64_value(c) else { return fail(out) };
        let remaining = max - out.bytes.len();
        if (remaining == 1 && chunk.len() == 2) || (remaining == 2 && chunk.len() == 3) {
            return out;
        }
        chunk.push(v);
        if chunk.len() == 4 {
            out.bytes.extend(decode_chunk(&chunk, false).unwrap_or_default());
            chunk.clear();
            out.read = i;
            if out.bytes.len() == max {
                return out;
            }
        }
    }
}

fn hex_value(c: u16) -> Option<u8> {
    let c = u8::try_from(c).ok()?;
    (c as char).to_digit(16).map(|d| d as u8)
}

/// FromHex.
fn from_hex_str(s: &[u16], max: usize) -> Decoded {
    let mut out = Decoded { read: 0, bytes: Vec::new(), error: false };
    if s.len() % 2 != 0 {
        out.error = true;
        return out;
    }
    while out.read < s.len() && out.bytes.len() < max {
        let (Some(h), Some(l)) = (hex_value(s[out.read]), hex_value(s[out.read + 1])) else {
            out.error = true;
            return out;
        };
        out.read += 2;
        out.bytes.push(h << 4 | l);
    }
    out
}

fn new_uint8(vm: &mut Vm, bytes: &[u8]) -> Result<Value, Value> {
    let o = super::typed::new_typed(vm, TaKind::U8, bytes.len(), None)?;
    if let Kind::TypedArray(t) = &o.borrow().kind {
        t.data.borrow_mut().copy_from_slice(bytes);
    }
    Ok(Value::Object(o))
}

fn from_base64(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = arg_string(vm, a)?;
    let opts = options(vm, &a.arg(1))?;
    let url = alphabet(vm, &opts)?;
    let last = last_chunk(vm, &opts)?;
    let d = from_base64_str(s.units(), url, last, usize::MAX);
    if d.error {
        return Err(vm.syntax_error("Invalid base64 string"));
    }
    new_uint8(vm, &d.bytes)
}

fn from_hex(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = arg_string(vm, a)?;
    let d = from_hex_str(s.units(), usize::MAX);
    if d.error {
        return Err(vm.syntax_error("Invalid hex string"));
    }
    new_uint8(vm, &d.bytes)
}

/// ValidateUint8Array para escrita.
fn this_uint8_write(vm: &mut Vm, a: &CallArgs, name: &str) -> Result<ObjRef, Value> {
    let o = this_uint8(vm, a)?;
    if o.borrow().ta_immutable() {
        return Err(vm.type_error(format!("Cannot perform Uint8Array.prototype.{name} on an immutable ArrayBuffer")));
    }
    Ok(o)
}

/// ValidateUint8Array.
fn this_uint8(vm: &mut Vm, a: &CallArgs) -> Result<ObjRef, Value> {
    if let Value::Object(o) = &a.this
        && matches!(&o.borrow().kind, Kind::TypedArray(t) if t.kind == TaKind::U8)
    {
        return Ok(o.clone());
    }
    Err(vm.type_error("this is not a Uint8Array"))
}

/// (bytes, offset, tamanho) de um Uint8Array dentro dos limites.
fn uint8_state(vm: &mut Vm, o: &ObjRef, name: &str) -> Result<(Bytes, usize, usize), Value> {
    if super::typed::ta_state(o).flatten().is_none() {
        return Err(vm.type_error(format!("Cannot perform Uint8Array.prototype.{name} on a detached ArrayBuffer")));
    }
    match &o.borrow().kind {
        Kind::TypedArray(t) => Ok((t.data.clone(), t.offset, t.len())),
        _ => Err(vm.type_error("this is not a Uint8Array")),
    }
}

fn to_base64(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_uint8(vm, a)?;
    let opts = options(vm, &a.arg(0))?;
    let url = alphabet(vm, &opts)?;
    let omit = truthy(&option(vm, &opts, "omitPadding")?);
    let (data, off, len) = uint8_state(vm, &o, "toBase64")?;
    let d = data.borrow();
    let bytes = &d[off..off + len];
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        let digits = c.len() + 1;
        for k in 0..4 {
            if k < digits {
                let mut ch = ALPHABET[(n >> (18 - 6 * k) & 63) as usize];
                if url {
                    ch = match ch {
                        b'+' => b'-',
                        b'/' => b'_',
                        x => x,
                    };
                }
                out.push(ch as char);
            } else if !omit {
                out.push('=');
            }
        }
    }
    Ok(Value::str(&out))
}

fn to_hex(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_uint8(vm, a)?;
    let (data, off, len) = uint8_state(vm, &o, "toHex")?;
    let d = data.borrow();
    let mut out = String::with_capacity(len * 2);
    for b in &d[off..off + len] {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(Value::str(&out))
}

/// Grava os bytes decodificados e devolve `{ read, written }` (ou o erro,
/// depois de gravar o que deu para decodificar).
fn write_decoded(vm: &mut Vm, data: &Bytes, off: usize, d: Decoded, what: &str) -> Result<Value, Value> {
    data.borrow_mut()[off..off + d.bytes.len()].copy_from_slice(&d.bytes);
    if d.error {
        return Err(vm.syntax_error(format!("Invalid {what} string")));
    }
    let r = vm.new_object();
    vm.define_data(&r, PropKey::str("read"), Value::Number(d.read as f64));
    vm.define_data(&r, PropKey::str("written"), Value::Number(d.bytes.len() as f64));
    Ok(Value::Object(r))
}

fn set_from_base64(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_uint8_write(vm, a, "setFromBase64")?;
    let s = arg_string(vm, a)?;
    let opts = options(vm, &a.arg(1))?;
    let url = alphabet(vm, &opts)?;
    let last = last_chunk(vm, &opts)?;
    let (data, off, len) = uint8_state(vm, &o, "setFromBase64")?;
    let d = from_base64_str(s.units(), url, last, len);
    write_decoded(vm, &data, off, d, "base64")
}

fn set_from_hex(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_uint8_write(vm, a, "setFromHex")?;
    let s = arg_string(vm, a)?;
    let (data, off, len) = uint8_state(vm, &o, "setFromHex")?;
    let d = from_hex_str(s.units(), len);
    write_decoded(vm, &data, off, d, "hex")
}
