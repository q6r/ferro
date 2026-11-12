//! Funções e valores globais: globalThis, isNaN, encodeURIComponent, eval...

use super::Realm;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let g = &r.global;
    r.global_value("globalThis", Value::Object(g.clone()));
    r.constant(g, "NaN", Value::Number(f64::NAN));
    r.constant(g, "Infinity", Value::Number(f64::INFINITY));
    r.constant(g, "undefined", Value::Undefined);
    for (name, len, f) in [
        ("isNaN", 1, is_nan as NativeFn),
        ("isFinite", 1, is_finite),
        ("encodeURIComponent", 1, encode_uri_component),
        ("encodeURI", 1, encode_uri),
        ("decodeURIComponent", 1, decode_uri_component),
        ("decodeURI", 1, decode_uri),
        ("escape", 1, escape),
        ("unescape", 1, unescape),
        ("eval", 1, eval),
    ] {
        r.method(g, name, len, f);
    }
}

fn is_nan(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(vm.to_number(&a.arg(0))?.is_nan()))
}

fn is_finite(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(vm.to_number(&a.arg(0))?.is_finite()))
}

const URI_UNRESERVED: &str = "-_.!~*'()";
const URI_RESERVED: &str = ";/?:@&=+$,#";

fn encode(vm: &mut Vm, s: &JsStr, keep: &str) -> Result<Value, Value> {
    let u = s.units();
    let mut out = String::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        let c = u[i];
        if c < 0x80 {
            let ch = c as u8 as char;
            if ch.is_ascii_alphanumeric() || URI_UNRESERVED.contains(ch) || keep.contains(ch) {
                out.push(ch);
                i += 1;
                continue;
            }
        }
        let (cp, n) = if (0xD800..0xDC00).contains(&c) {
            if i + 1 < u.len() && (0xDC00..0xE000).contains(&u[i + 1]) {
                (0x10000 + (((c as u32) - 0xD800) << 10) + (u[i + 1] as u32 - 0xDC00), 2)
            } else {
                return Err(uri_error(vm));
            }
        } else if (0xDC00..0xE000).contains(&c) {
            return Err(uri_error(vm));
        } else {
            (c as u32, 1)
        };
        let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
        let mut buf = [0u8; 4];
        for b in ch.encode_utf8(&mut buf).bytes() {
            out.push_str(&format!("%{b:02X}"));
        }
        i += n;
    }
    Ok(Value::str(&out))
}

fn uri_error(vm: &mut Vm) -> Value {
    let p = vm.realm.uri_error_proto.clone();
    vm.make_error(&p, "URI malformed")
}

fn decode(vm: &mut Vm, s: &JsStr, reserved: &str) -> Result<Value, Value> {
    let u = s.units();
    let mut out: Vec<u16> = Vec::with_capacity(u.len());
    let mut i = 0;
    let hex = |c: u16| char::from_u32(c as u32).and_then(|ch| ch.to_digit(16));
    while i < u.len() {
        if u[i] != b'%' as u16 {
            out.push(u[i]);
            i += 1;
            continue;
        }
        let byte_at = |k: usize| -> Option<u8> {
            if u.get(k) != Some(&(b'%' as u16)) {
                return None;
            }
            let h = hex(*u.get(k + 1)?)?;
            let l = hex(*u.get(k + 2)?)?;
            Some((h * 16 + l) as u8)
        };
        let Some(b0) = byte_at(i) else { return Err(uri_error(vm)) };
        if b0 < 0x80 {
            let ch = b0 as char;
            if reserved.contains(ch) {
                out.extend(u[i..i + 3].iter());
            } else {
                out.push(b0 as u16);
            }
            i += 3;
            continue;
        }
        let n = if b0 & 0xE0 == 0xC0 {
            2
        } else if b0 & 0xF0 == 0xE0 {
            3
        } else if b0 & 0xF8 == 0xF0 {
            4
        } else {
            return Err(uri_error(vm));
        };
        let mut bytes = vec![b0];
        for k in 1..n {
            let Some(b) = byte_at(i + 3 * k) else { return Err(uri_error(vm)) };
            bytes.push(b);
        }
        match std::str::from_utf8(&bytes) {
            Ok(s) => out.extend(s.encode_utf16()),
            Err(_) => return Err(uri_error(vm)),
        }
        i += 3 * n;
    }
    Ok(Value::String(JsStr::from_units(out)))
}

