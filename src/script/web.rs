//! O resto da plataforma web em Rust: rede, URL, armazenamento, cookies,
//! base64, medidas da janela. A maior parte fica num objeto `__native` que
//! o prelúdio usa e depois esconde.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use ferro_js::value::{CallArgs, Kind, ObjRef, PropKey, TaKind};
use ferro_js::{Value, Vm};
use url::Url;

use super::{Done, Host, host, node_of};
use crate::net;

type R = Result<Value, Value>;

fn s(v: &str) -> Value {
    Value::str(v)
}

fn read<T>(vm: &Vm, f: impl FnOnce(&Host) -> T) -> T {
    let h = host(vm);
    let b = h.borrow();
    f(&b)
}

fn write<T>(vm: &Vm, f: impl FnOnce(&mut Host) -> T) -> T {
    let h = host(vm);
    let mut b = h.borrow_mut();
    f(&mut b)
}

fn arg_string(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<String, Value> {
    vm.to_rust_string(&a.arg(i))
}

fn set(vm: &mut Vm, o: &ObjRef, k: &str, v: Value) {
    vm.define_data(o, PropKey::str(k), v);
}

// ------------------------------------------------------------------ bytes

/// Os bytes de um TypedArray, ArrayBuffer ou DataView.
pub(crate) fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    let Value::Object(o) = v else { return None };
    let b = o.borrow();
    match &b.kind {
        Kind::ArrayBuffer(bytes) => Some(bytes.borrow().clone()),
        Kind::TypedArray(t) | Kind::DataView(t) => {
            let data = t.data.borrow();
            let len = if matches!(b.kind, Kind::DataView(_)) { t.length } else { t.length * t.kind.size() };
            let end = (t.offset + len).min(data.len());
            Some(data[t.offset.min(end)..end].to_vec())
        }
        _ => None,
    }
}

fn uint8_array(vm: &mut Vm, bytes: &[u8]) -> R {
    let o = ferro_js::builtins::typed::new_typed(vm, TaKind::U8, bytes.len(), None)?;
    if let Kind::TypedArray(t) = &o.borrow().kind {
        t.data.borrow_mut().copy_from_slice(bytes);
    }
    Ok(Value::Object(o))
}

fn utf8_encode(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = if a.arg(0).is_undefined() { String::new() } else { arg_string(vm, a, 0)? };
    uint8_array(vm, text.as_bytes())
}

fn utf8_decode(vm: &mut Vm, a: &mut CallArgs) -> R {
    let bytes = bytes_of(&a.arg(0)).unwrap_or_default();
    let label = if a.arg(1).is_undefined() { "utf-8".to_string() } else { arg_string(vm, a, 1)? };
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    Ok(s(&net::decode_text(bytes, Some(&format!("text/plain; charset={label}")), false)))
}

// ----------------------------------------------------------------- base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn btoa(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = arg_string(vm, a, 0)?;
    let mut bytes = Vec::with_capacity(text.len());
    for c in text.chars() {
        if c as u32 > 255 {
            return Err(super::dom::dom_error(
                vm,
                "InvalidCharacterError",
                "Failed to execute 'btoa' on 'Window': The string to be encoded contains characters outside of the Latin1 range.",
            ));
        }
        bytes.push(c as u8);
    }
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    Ok(s(&out))
}

fn atob(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = arg_string(vm, a, 0)?;
    let clean: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let clean = if clean.len() % 4 == 0 {
        let mut c = clean;
        while c.last() == Some(&b'=') {
            c.pop();
        }
        c
    } else {
        clean
    };
    let bad = || "Failed to execute 'atob' on 'Window': The string to be decoded is not correctly encoded.";
    if clean.len() % 4 == 1 {
        return Err(super::dom::dom_error(vm, "InvalidCharacterError", bad()));
    }
    let mut out = String::with_capacity(clean.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in &clean {
        let v = match B64.iter().position(|&x| x == c) {
            Some(v) => v as u32,
            None => return Err(super::dom::dom_error(vm, "InvalidCharacterError", bad())),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(char::from((acc >> bits) as u8));
            acc &= (1 << bits) - 1;
        }
    }
    Ok(s(&out))
}

// -------------------------------------------------------------------- rede

/// Objeto com o resultado de uma requisição (o prelúdio monta o Response).
pub(crate) fn response_value(vm: &mut Vm, res: Result<net::Response, String>) -> Value {
    let o = vm.new_object();
    match res {
        Ok(r) => {
            set(vm, &o, "status", Value::Number(r.status as f64));
            set(vm, &o, "url", s(r.url.as_str()));
            let headers: Vec<Value> = r
                .headers
                .iter()
                .filter(|(k, _)| k != "set-cookie")
                .map(|(k, v)| vm.array_value(vec![s(k), s(v)]))
                .collect();
            let h = vm.array_value(headers);
            set(vm, &o, "headers", h);
            let bytes = uint8_array(vm, &r.body).unwrap_or(Value::Null);
            set(vm, &o, "body", bytes);
            set(vm, &o, "contentType", r.content_type.as_deref().map_or(Value::Null, s));
        }
        Err(e) => {
            set(vm, &o, "error", s(&e));
        }
    }
    Value::Object(o)
}

