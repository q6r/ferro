//! String, String.prototype e o iterador de strings.

use super::{Realm, rel_index};
use super::array::iter_result;
use crate::string::{JsStr, is_js_space, trim_js};
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let proto = &r.string_proto;
    let c = r.ctor("String", 1, string_ctor, proto);
    r.method(&c, "fromCharCode", 1, from_char_code);
    r.method(&c, "fromCodePoint", 1, from_code_point);
    r.method(&c, "raw", 1, raw);

    r.method(proto, "at", 1, at);
    r.method(proto, "charAt", 1, char_at);
    r.method(proto, "charCodeAt", 1, char_code_at);
    r.method(proto, "codePointAt", 1, code_point_at);
    r.method(proto, "concat", 1, concat);
    r.method(proto, "endsWith", 1, ends_with);
    r.method(proto, "includes", 1, includes);
    r.method(proto, "indexOf", 1, index_of);
    r.method(proto, "lastIndexOf", 1, last_index_of);
    r.method(proto, "localeCompare", 1, locale_compare);
    r.method(proto, "normalize", 0, normalize);
    r.method(proto, "padEnd", 1, pad_end);
    r.method(proto, "padStart", 1, pad_start);
    r.method(proto, "repeat", 1, repeat);
    r.method(proto, "replace", 2, replace);
    r.method(proto, "replaceAll", 2, replace_all);
    r.method(proto, "slice", 2, slice);
    r.method(proto, "split", 2, split);
    r.method(proto, "startsWith", 1, starts_with);
    r.method(proto, "substring", 2, substring);
    r.method(proto, "substr", 2, substr);
    r.method(proto, "toLowerCase", 0, to_lower);
    r.method(proto, "toUpperCase", 0, to_upper);
    r.method(proto, "toLocaleLowerCase", 0, to_lower);
    r.method(proto, "toLocaleUpperCase", 0, to_upper);
    r.method(proto, "toString", 0, to_string);
    r.method(proto, "valueOf", 0, to_string);
    let trim_f = r.method(proto, "trim", 0, trim);
    let _ = trim_f;
    let ts = r.method(proto, "trimStart", 0, trim_start);
    let te = r.method(proto, "trimEnd", 0, trim_end);
    proto.borrow_mut().props.insert(PropKey::str("trimLeft"), Property::hidden(Value::Object(ts)));
    proto.borrow_mut().props.insert(PropKey::str("trimRight"), Property::hidden(Value::Object(te)));
    r.method(proto, "match", 1, match_);
    r.method(proto, "matchAll", 1, match_all);
    r.method(proto, "search", 1, search);
    r.method(proto, "isWellFormed", 0, is_well_formed);
    r.method(proto, "toWellFormed", 0, to_well_formed);
    r.method_sym(proto, &r.sym_iterator, "[Symbol.iterator]", 0, iterator);

    let ip = &r.string_iter_proto;
    r.method(ip, "next", 0, iter_next);
    r.tag(ip, "String Iterator");
}

fn string_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = if a.args.is_empty() {
        JsStr::empty()
    } else {
        match &a.args[0] {
            Value::Symbol(s) if a.new_target.is_none() => {
                JsStr::new(&format!("Symbol({})", s.description().map(|d| d.to_rust()).unwrap_or_default()))
            }
            v => vm.to_string(v)?,
        }
    };
    match &a.new_target {
        None => Ok(Value::String(s)),
        Some(nt) => {
            let def = vm.realm.string_proto.clone();
            let proto = vm.proto_from_ctor(Some(nt), &def)?;
            Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::String(s)))))
        }
    }
}

/// `this` como string (RequireObjectCoercible + ToString).
pub fn this_str(vm: &mut Vm, a: &CallArgs, what: &str) -> Result<JsStr, Value> {
    match &a.this {
        Value::String(s) => Ok(s.clone()),
        Value::Undefined | Value::Null => Err(vm.type_error(format!("String.prototype.{what} called on null or undefined"))),
        v => vm.to_string(v),
    }
}

fn from_char_code(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut out = Vec::with_capacity(a.args.len());
    for v in &a.args {
        out.push(crate::conv::to_uint16(vm.to_number(v)?));
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn from_code_point(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut out = Vec::new();
    for v in &a.args {
        let n = vm.to_number(v)?;
        if n.fract() != 0.0 || !(0.0..=1114111.0).contains(&n) {
            return Err(vm.range_error(format!("Invalid code point {}", crate::conv::number_to_string(n))));
        }
        push_code_point(&mut out, n as u32);
    }
    Ok(Value::String(JsStr::from_units(out)))
}

pub fn push_code_point(out: &mut Vec<u16>, cp: u32) {
    if cp < 0x10000 {
        out.push(cp as u16);
    } else {
        let c = cp - 0x10000;
        out.push(0xD800 + (c >> 10) as u16);
        out.push(0xDC00 + (c & 0x3FF) as u16);
    }
}

