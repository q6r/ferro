//! RegExp: a API do JavaScript sobre o motor de `crate::regex`.

use std::rc::Rc;

use super::array::iter_result;
use super::strings::get_substitution;
use super::{Realm, native_fn};
use crate::regex::{Flags, NONE, Program};
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Vm, same_value, truthy};

pub fn install(r: &Realm) {
    let p = &r.regexp_proto;
    let c = r.ctor("RegExp", 2, regexp_ctor, p);
    r.getter_sym(&c, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.method(&c, "escape", 1, escape);
    r.getter(p, "source", source);
    r.getter(p, "flags", flags);
    for (name, f) in [
        ("global", flag_g as NativeFn),
        ("ignoreCase", flag_i),
        ("multiline", flag_m),
        ("dotAll", flag_s),
        ("unicode", flag_u),
        ("unicodeSets", flag_v),
        ("sticky", flag_y),
        ("hasIndices", flag_d),
    ] {
        r.getter(p, name, f);
    }
    r.method(p, "toString", 0, to_string);
    p.borrow_mut().props.insert(PropKey::str("exec"), Property::hidden(Value::Object(r.regexp_exec.clone())));
    r.method(p, "test", 1, test);
    r.method(p, "compile", 2, compile_legacy);
    r.method_sym(p, &r.sym_match, "[Symbol.match]", 1, sym_match);
    r.method_sym(p, &r.sym_match_all, "[Symbol.matchAll]", 1, sym_match_all);
    r.method_sym(p, &r.sym_replace, "[Symbol.replace]", 2, sym_replace);
    r.method_sym(p, &r.sym_search, "[Symbol.search]", 1, sym_search);
    r.method_sym(p, &r.sym_split, "[Symbol.split]", 2, sym_split);
    let ip = &r.regexp_string_iter_proto;
    r.method(ip, "next", 0, string_iter_next);
    r.tag(ip, "RegExp String Iterator");
}

pub fn exec_native(function_proto: &ObjRef) -> ObjRef {
    native_fn(function_proto, "exec", 1, exec, false)
}

impl Vm {
    /// Compila (com cache) um padrão; erros viram SyntaxError como no V8.
    pub fn compile_regex(&mut self, source: &JsStr, flags: &JsStr) -> Result<Rc<Program>, Value> {
        let key = (source.clone(), flags.clone());
        if let Some(p) = self.regex_cache.get(&key) {
            return Ok(p.clone());
        }
        let f = flags.to_rust();
        let Some(fl) = Flags::parse(&f) else {
            return Err(self.syntax_error(format!("Invalid flags supplied to RegExp constructor '{f}'")));
        };
        match crate::regex::compile(source.units(), fl) {
            Ok(p) => {
                let p = Rc::new(p);
                if self.regex_cache.len() > 2000 {
                    self.regex_cache.clear();
                }
                self.regex_cache.insert(key, p.clone());
                Ok(p)
            }
            Err(e) => Err(self.syntax_error(format!("Invalid regular expression: /{}/{}: {}", source.to_rust(), f, e.0))),
        }
    }
}

/// Cria um RegExp (usado por literais `/.../` e pelo construtor).
pub fn create(vm: &mut Vm, source: JsStr, flags: JsStr, new_target: Option<&ObjRef>) -> Result<Value, Value> {
    let prog = vm.compile_regex(&source, &flags)?;
    let def = vm.realm.regexp_proto.clone();
    let proto = vm.proto_from_ctor(new_target, &def)?;
    let o = ObjRef::new(Object::new(Some(proto), Kind::RegExp { source, flags, prog }));
    o.borrow_mut().props.insert(
        PropKey::str("lastIndex"),
        Property {
            slot: Slot::Data {
                value: Value::Number(0.0),
                writable: true,
            },
            enumerable: false,
            configurable: false,
        },
    );
    Ok(Value::Object(o))
}

fn regexp_parts(v: &Value) -> Option<(JsStr, JsStr, Rc<Program>)> {
    match v {
        Value::Object(o) => match &o.borrow().kind {
            Kind::RegExp { source, flags, prog } => Some((source.clone(), flags.clone(), prog.clone())),
            _ => None,
        },
        _ => None,
    }
}

fn regexp_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let pat = a.arg(0);
    let fl = a.arg(1);
    let existing = regexp_parts(&pat);
    if a.new_target.is_none()
        && existing.is_some()
        && fl.is_undefined()
        && let Value::Object(o) = &pat
    {
        let ctor = vm.get_obj(o, &PropKey::str("constructor"))?;
        if let Value::Object(c) = ctor
            && c.ptr_eq(&a.callee)
        {
            return Ok(pat);
        }
    }
    let (src, flags) = match existing {
        Some((s, f, _)) => (s, if fl.is_undefined() { f } else { vm.to_string(&fl)? }),
        None => {
            let s = if pat.is_undefined() { JsStr::empty() } else { vm.to_string(&pat)? };
            let f = if fl.is_undefined() { JsStr::empty() } else { vm.to_string(&fl)? };
            (s, f)
        }
    };
    let nt = a.new_target.clone();
    create(vm, src, flags, nt.as_ref())
}

fn escape(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::String(s) = a.arg(0) else {
        return Err(vm.type_error("RegExp.escape requires a string"));
    };
    let mut out = String::new();
    for (i, c) in s.to_rust().chars().enumerate() {
        if i == 0 && c.is_ascii_alphanumeric() {
            out.push_str(&format!("\\x{:02x}", c as u32));
        } else if "^$\\.*+?()[]{}|/".contains(c) {
            out.push('\\');
            out.push(c);
        } else if ",-=<>#&!%:;@~'`\"".contains(c) || c.is_whitespace() {
            if (c as u32) < 0x100 {
                out.push_str(&format!("\\x{:02x}", c as u32));
            } else {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
        } else {
            out.push(c);
        }
    }
    Ok(Value::str(&out))
}

fn this_parts(vm: &mut Vm, a: &CallArgs, what: &str) -> Result<(JsStr, JsStr, Rc<Program>), Value> {
    regexp_parts(&a.this).ok_or_else(|| {
        let d = vm.describe(&a.this);
        vm.type_error(format!("RegExp.prototype.{what} requires that 'this' be a RegExp object (got {d})"))
    })
}

/// O `source` escapado como o spec pede (`/` vira `\/`, quebras viram `\n`).
fn escape_source(src: &JsStr) -> JsStr {
    if src.is_empty() {
        return JsStr::new("(?:)");
    }
    let mut out: Vec<u16> = Vec::with_capacity(src.len());
    let mut in_class = false;
    let mut escaped = false;
    for &c in src.units() {
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        match c {
            0x5C => {
                escaped = true;
                out.push(c);
            }
            0x5B => {
                in_class = true;
                out.push(c);
            }
            0x5D => {
                in_class = false;
                out.push(c);
            }
            0x2F if !in_class => out.extend_from_slice(&[0x5C, 0x2F]),
            0x0A => out.extend("\\n".encode_utf16()),
            0x0D => out.extend("\\r".encode_utf16()),
            0x2028 => out.extend("\\u2028".encode_utf16()),
            0x2029 => out.extend("\\u2029".encode_utf16()),
            _ => out.push(c),
        }
    }
    JsStr::from_units(out)
}

fn is_proto(vm: &Vm, v: &Value) -> bool {
    matches!(v, Value::Object(o) if o.ptr_eq(&vm.realm.regexp_proto))
}

fn source(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if is_proto(vm, &a.this) {
        return Ok(Value::str("(?:)"));
    }
    Ok(Value::String(escape_source(&this_parts(vm, a, "source")?.0)))
}

fn flags(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if !matches!(a.this, Value::Object(_)) {
        return Err(vm.type_error("RegExp.prototype.flags getter called on non-object"));
    }
    let mut out = String::new();
    for (c, name) in [
        ('d', "hasIndices"),
        ('g', "global"),
        ('i', "ignoreCase"),
        ('m', "multiline"),
        ('s', "dotAll"),
        ('u', "unicode"),
        ('v', "unicodeSets"),
        ('y', "sticky"),
    ] {
        if truthy(&vm.get_str(&a.this, name)?) {
            out.push(c);
        }
    }
    Ok(Value::str(&out))
}

fn has_flag(vm: &mut Vm, a: &CallArgs, c: char, name: &str) -> Result<Value, Value> {
    if is_proto(vm, &a.this) {
        return Ok(Value::Undefined);
    }
    Ok(Value::Bool(this_parts(vm, a, name)?.1.to_rust().contains(c)))
}

fn flag_g(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    has_flag(vm, a, 'g', "global")
}
fn flag_i(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    has_flag(vm, a, 'i', "ignoreCase")
}
fn flag_m(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    has_flag(vm, a, 'm', "multiline")
}
fn flag_s(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    has_flag(vm, a, 's', "dotAll")
}
fn flag_u(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    has_flag(vm, a, 'u', "unicode")
}
fn flag_v(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    has_flag(vm, a, 'v', "unicodeSets")
}
fn flag_y(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    has_flag(vm, a, 'y', "sticky")
}
fn flag_d(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    has_flag(vm, a, 'd', "hasIndices")
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if !matches!(a.this, Value::Object(_)) {
        return Err(vm.type_error("RegExp.prototype.toString requires that 'this' be an Object"));
    }
    let s = vm.get_str(&a.this, "source")?;
    let f = vm.get_str(&a.this, "flags")?;
    let s = vm.to_rust_string(&s)?;
    let f = vm.to_rust_string(&f)?;
    Ok(Value::str(&format!("/{s}/{f}")))
}

