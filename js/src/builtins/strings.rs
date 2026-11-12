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

fn raw(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let strs = vm.to_object(&a.arg(0))?;
    let raw = vm.get_obj(&strs, &PropKey::str("raw"))?;
    let raw = vm.to_object(&raw)?;
    let len = vm.length_of(&raw)?;
    let mut out = Vec::new();
    for i in 0..len {
        let part = vm.get_obj(&raw, &crate::ops::index_key(i))?;
        out.extend_from_slice(vm.to_string(&part)?.units());
        if i + 1 < len && i + 1 < a.args.len() {
            out.extend_from_slice(vm.to_string(&a.args[i + 1])?.units());
        }
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn at(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "at")?;
    let n = vm.to_integer(&a.arg(0))?;
    let len = s.len() as f64;
    let k = if n >= 0.0 { n } else { len + n };
    if k < 0.0 || k >= len {
        return Ok(Value::Undefined);
    }
    Ok(Value::String(s.slice(k as usize, k as usize + 1)))
}

fn char_at(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "charAt")?;
    let n = vm.to_integer(&a.arg(0))?;
    if n < 0.0 || n >= s.len() as f64 {
        return Ok(Value::str(""));
    }
    Ok(Value::String(s.slice(n as usize, n as usize + 1)))
}

fn char_code_at(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "charCodeAt")?;
    let n = vm.to_integer(&a.arg(0))?;
    if n < 0.0 || n >= s.len() as f64 {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number(s.units()[n as usize] as f64))
}

pub fn code_point_at_units(u: &[u16], i: usize) -> (u32, usize) {
    let c = u[i];
    if (0xD800..0xDC00).contains(&c) && i + 1 < u.len() && (0xDC00..0xE000).contains(&u[i + 1]) {
        let cp = 0x10000 + (((c as u32) - 0xD800) << 10) + (u[i + 1] as u32 - 0xDC00);
        (cp, 2)
    } else {
        (c as u32, 1)
    }
}

fn code_point_at(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "codePointAt")?;
    let n = vm.to_integer(&a.arg(0))?;
    if n < 0.0 || n >= s.len() as f64 {
        return Ok(Value::Undefined);
    }
    Ok(Value::Number(code_point_at_units(s.units(), n as usize).0 as f64))
}

fn concat(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut s = this_str(vm, a, "concat")?.units().to_vec();
    for v in &a.args {
        s.extend_from_slice(vm.to_string(v)?.units());
    }
    Ok(Value::String(JsStr::from_units(s)))
}

fn reject_regexp(vm: &mut Vm, v: &Value, what: &str) -> Result<(), Value> {
    if let Value::Object(o) = v
        && matches!(o.borrow().kind, Kind::RegExp { .. })
    {
        return Err(vm.type_error(format!("First argument to String.prototype.{what} must not be a regular expression")));
    }
    Ok(())
}

fn ends_with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "endsWith")?;
    reject_regexp(vm, &a.arg(0), "endsWith")?;
    let search = vm.to_string(&a.arg(0))?;
    let end = rel_clamp(vm, &a.arg(1), s.len(), s.len())?;
    if search.len() > end {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(&s.units()[end - search.len()..end] == search.units()))
}

/// Posição absoluta (sem índices negativos), limitada ao tamanho.
fn rel_clamp(vm: &mut Vm, v: &Value, len: usize, default: usize) -> Result<usize, Value> {
    if v.is_undefined() {
        return Ok(default);
    }
    let n = vm.to_integer(v)?;
    Ok(n.clamp(0.0, len as f64) as usize)
}

fn starts_with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "startsWith")?;
    reject_regexp(vm, &a.arg(0), "startsWith")?;
    let search = vm.to_string(&a.arg(0))?;
    let start = rel_clamp(vm, &a.arg(1), s.len(), 0)?;
    if start + search.len() > s.len() {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(&s.units()[start..start + search.len()] == search.units()))
}

fn includes(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "includes")?;
    reject_regexp(vm, &a.arg(0), "includes")?;
    let search = vm.to_string(&a.arg(0))?;
    let start = rel_clamp(vm, &a.arg(1), s.len(), 0)?;
    Ok(Value::Bool(s.find(&search, start).is_some()))
}

fn index_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "indexOf")?;
    let search = vm.to_string(&a.arg(0))?;
    let start = rel_clamp(vm, &a.arg(1), s.len(), 0)?;
    Ok(Value::Number(s.find(&search, start).map_or(-1.0, |i| i as f64)))
}

fn last_index_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "lastIndexOf")?;
    let search = vm.to_string(&a.arg(0))?;
    let n = vm.to_number(&a.arg(1))?;
    let from = if n.is_nan() { s.len() } else { crate::conv::to_integer(n).clamp(0.0, s.len() as f64) as usize };
    Ok(Value::Number(s.rfind(&search, from).map_or(-1.0, |i| i as f64)))
}

fn locale_compare(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "localeCompare")?.to_rust();
    let t = vm.to_rust_string(&a.arg(0))?;
    let opts = a.arg(2);
    let (mut numeric, mut sens) = (false, "variant".to_string());
    if let Value::Object(_) = &opts {
        numeric = crate::vm::truthy(&vm.get_str(&opts, "numeric")?);
        let sv = vm.get_str(&opts, "sensitivity")?;
        if !sv.is_undefined() {
            sens = vm.to_rust_string(&sv)?;
        }
    }
    let ord = super::intl::collate(&s, &t, numeric, &sens);
    Ok(Value::Number(match ord {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

fn normalize(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "normalize")?;
    let form = if a.arg(0).is_undefined() { "NFC".to_string() } else { vm.to_rust_string(&a.arg(0))? };
    if !matches!(form.as_str(), "NFC" | "NFD" | "NFKC" | "NFKD") {
        return Err(vm.range_error(format!("The normalization form should be one of NFC, NFD, NFKC, NFKD.")));
    }
    Ok(Value::String(s))
}

fn pad(vm: &mut Vm, a: &CallArgs, at_start: bool) -> Result<Value, Value> {
    let s = this_str(vm, a, if at_start { "padStart" } else { "padEnd" })?;
    let max = vm.to_integer(&a.arg(0))?;
    let fill = if a.arg(1).is_undefined() { JsStr::new(" ") } else { vm.to_string(&a.arg(1))? };
    if max <= s.len() as f64 || fill.is_empty() {
        return Ok(Value::String(s));
    }
    if max > 536870888.0 {
        return Err(vm.range_error("Invalid string length"));
    }
    let need = max as usize - s.len();
    let mut filler: Vec<u16> = Vec::with_capacity(need);
    while filler.len() < need {
        let take = (need - filler.len()).min(fill.len());
        filler.extend_from_slice(&fill.units()[..take]);
    }
    let out = if at_start {
        let mut v = filler;
        v.extend_from_slice(s.units());
        v
    } else {
        let mut v = s.units().to_vec();
        v.extend(filler);
        v
    };
    Ok(Value::String(JsStr::from_units(out)))
}

fn pad_start(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    pad(vm, a, true)
}

fn pad_end(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    pad(vm, a, false)
}

fn repeat(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = this_str(vm, a, "repeat")?;
    let n = vm.to_integer(&a.arg(0))?;
    if n < 0.0 || n.is_infinite() {
        return Err(vm.range_error(format!("Invalid count value: {}", crate::conv::number_to_string(n))));
    }
    if s.len() as f64 * n > 536870888.0 {
        return Err(vm.range_error("Invalid string length"));
    }
    Ok(Value::String(JsStr::from_units(s.units().repeat(n as usize))))
}

/// GetSubstitution: expande `$&`, `$1`, `$<nome>` etc. na substituição.
pub fn get_substitution(
    vm: &mut Vm,
    matched: &JsStr,
    s: &JsStr,
    pos: usize,
    captures: &[Value],
    groups: &Value,
    replacement: &JsStr,
) -> Result<JsStr, Value> {
    let r = replacement.units();
    let mut out: Vec<u16> = Vec::with_capacity(r.len());
    let m = captures.len();
    let mut i = 0;
    while i < r.len() {
        let c = r[i];
        if c != b'$' as u16 || i + 1 >= r.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let n = r[i + 1];
        match n {
            0x24 => {
                out.push(0x24);
                i += 2;
            }
            0x26 => {
                out.extend_from_slice(matched.units());
                i += 2;
            }
            0x60 => {
                out.extend_from_slice(&s.units()[..pos]);
                i += 2;
            }
            0x27 => {
                let end = (pos + matched.len()).min(s.len());
                out.extend_from_slice(&s.units()[end..]);
                i += 2;
            }
            0x30..=0x39 => {
                let d1 = (n - 0x30) as usize;
                let two = if i + 2 < r.len() && (0x30..=0x39).contains(&r[i + 2]) {
                    Some(d1 * 10 + (r[i + 2] - 0x30) as usize)
                } else {
                    None
                };
                let (idx, adv) = match two {
                    Some(t) if t >= 1 && t <= m => (t, 3),
                    _ => (d1, 2),
                };
                if idx >= 1 && idx <= m {
                    if let Value::String(cap) = &captures[idx - 1] {
                        out.extend_from_slice(cap.units());
                    }
                    i += adv;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            0x3C => {
                if groups.is_undefined() {
                    out.push(c);
                    i += 1;
                    continue;
                }
                let close = r[i + 2..].iter().position(|&x| x == 0x3E);
                match close {
                    Some(p) => {
                        let name = JsStr::from_slice(&r[i + 2..i + 2 + p]);
                        let v = vm.get(groups, &PropKey::Str(name))?;
                        if !v.is_undefined() {
                            out.extend_from_slice(vm.to_string(&v)?.units());
                        }
                        i += p + 3;
                    }
                    None => {
                        out.push(c);
                        i += 1;
                    }
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(JsStr::from_units(out))
}

