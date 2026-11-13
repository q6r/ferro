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
    let ma = r.method_sym(p, &r.sym_match_all, "[Symbol.matchAll]", 1, sym_match_all);
    super::set_native_data(&ma, Value::Object(c.clone()));
    r.method_sym(p, &r.sym_replace, "[Symbol.replace]", 2, sym_replace);
    r.method_sym(p, &r.sym_search, "[Symbol.search]", 1, sym_search);
    let sp = r.method_sym(p, &r.sym_split, "[Symbol.split]", 2, sym_split);
    super::set_native_data(&sp, Value::Object(c.clone()));
    let ip = &r.regexp_string_iter_proto;
    r.method(ip, "next", 0, string_iter_next);
    r.tag(ip, "RegExp String Iterator");

    // Anexo B: RegExp.$1..$9, input/$_, lastMatch/$&, lastParen/$+,
    // leftContext/$`, rightContext/$' (do último casamento)
    let caps: [NativeFn; 9] = [cap1, cap2, cap3, cap4, cap5, cap6, cap7, cap8, cap9];
    for (i, f) in caps.into_iter().enumerate() {
        legacy_accessor(r, &c, &format!("${}", i + 1), f, None);
    }
    for (names, get, set) in [
        (["input", "$_"], legacy_input as NativeFn, Some(legacy_set_input as NativeFn)),
        (["lastMatch", "$&"], legacy_last_match, None),
        (["lastParen", "$+"], legacy_last_paren, None),
        (["leftContext", "$`"], legacy_left, None),
        (["rightContext", "$'"], legacy_right, None),
    ] {
        for n in names {
            legacy_accessor(r, &c, n, get, set);
        }
    }
}

fn legacy_accessor(r: &Realm, c: &ObjRef, name: &str, get: NativeFn, set: Option<NativeFn>) {
    let g = r.native(&format!("get {name}"), 0, get);
    super::set_native_data(&g, Value::Object(c.clone()));
    let s = set.map(|f| {
        let s = r.native(&format!("set {name}"), 1, f);
        super::set_native_data(&s, Value::Object(c.clone()));
        s
    });
    c.borrow_mut().props.insert(
        PropKey::str(name),
        Property {
            slot: Slot::Accessor { get: Some(g), set: s },
            enumerable: false,
            configurable: true,
        },
    );
}

/// Os acessores só valem com o próprio RegExp como `this`.
fn legacy_check(vm: &mut Vm, a: &CallArgs) -> Result<(), Value> {
    match (&a.this, super::native_data(&a.callee)) {
        (Value::Object(t), Value::Object(c)) if t.ptr_eq(&c) => Ok(()),
        _ => Err(vm.type_error("RegExp legacy static accessor called on incompatible receiver")),
    }
}

fn legacy_group(vm: &mut Vm, a: &CallArgs, g: usize) -> Result<Value, Value> {
    legacy_check(vm, a)?;
    let (s, caps) = &vm.regexp_statics;
    let (x, y) = (caps.get(2 * g).copied().unwrap_or(NONE), caps.get(2 * g + 1).copied().unwrap_or(NONE));
    Ok(Value::String(if x == NONE || y == NONE { JsStr::empty() } else { s.slice(x, y) }))
}

macro_rules! cap_fns {
    ($($name:ident = $g:expr),*) => {
        $(fn $name(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
            legacy_group(vm, a, $g)
        })*
    };
}
cap_fns!(cap1 = 1, cap2 = 2, cap3 = 3, cap4 = 4, cap5 = 5, cap6 = 6, cap7 = 7, cap8 = 8, cap9 = 9);

fn legacy_input(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    legacy_check(vm, a)?;
    Ok(Value::String(vm.regexp_statics.0.clone()))
}

fn legacy_set_input(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    legacy_check(vm, a)?;
    let s = vm.to_string(&a.arg(0))?;
    vm.regexp_statics.0 = s;
    Ok(Value::Undefined)
}

fn legacy_last_match(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    legacy_group(vm, a, 0)
}

fn legacy_last_paren(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let n = vm.regexp_statics.1.len() / 2;
    if n <= 1 {
        legacy_check(vm, a)?;
        return Ok(Value::String(JsStr::empty()));
    }
    legacy_group(vm, a, n - 1)
}

fn legacy_left(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    legacy_check(vm, a)?;
    let (s, caps) = &vm.regexp_statics;
    let start = caps.first().copied().unwrap_or(0).min(s.len());
    Ok(Value::String(s.slice(0, start)))
}

fn legacy_right(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    legacy_check(vm, a)?;
    let (s, caps) = &vm.regexp_statics;
    let end = caps.get(1).copied().unwrap_or(0).min(s.len());
    Ok(Value::String(s.slice(end, s.len())))
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
    Ok(Value::Object(alloc(proto, source, flags, prog)))
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

/// IsRegExp: objeto com `Symbol.match` verdadeiro (ou, sem ele, um RegExp de verdade).
fn is_regexp(vm: &mut Vm, v: &Value) -> Result<bool, Value> {
    let Value::Object(o) = v else { return Ok(false) };
    let m = vm.get_obj(o, &PropKey::Sym(vm.realm.sym_match.clone()))?;
    if !m.is_undefined() {
        return Ok(truthy(&m));
    }
    Ok(matches!(o.borrow().kind, Kind::RegExp { .. }))
}

fn regexp_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let pat = a.arg(0);
    let fl = a.arg(1);
    let pattern_is_regexp = is_regexp(vm, &pat)?;
    if a.new_target.is_none() && pattern_is_regexp && fl.is_undefined() {
        let ctor = vm.get_str(&pat, "constructor")?;
        if let Value::Object(c) = ctor
            && c.ptr_eq(&a.callee)
        {
            return Ok(pat);
        }
    }
    // padrão e flags: de um RegExp, de um objeto "regexp-like" ou como vieram
    let (p, f) = if let Some((s, f0, _)) = regexp_parts(&pat) {
        (Value::String(s), if fl.is_undefined() { Value::String(f0) } else { fl })
    } else if pattern_is_regexp {
        let s = vm.get_str(&pat, "source")?;
        let f = if fl.is_undefined() { vm.get_str(&pat, "flags")? } else { fl };
        (s, f)
    } else {
        (pat, fl)
    };
    // RegExpAlloc (o protótipo vem antes das conversões) e RegExpInitialize
    let nt = a.new_target.clone().unwrap_or_else(|| a.callee.clone());
    let def = vm.realm.regexp_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let src = if p.is_undefined() { JsStr::empty() } else { vm.to_string(&p)? };
    let flags = if f.is_undefined() { JsStr::empty() } else { vm.to_string(&f)? };
    let prog = vm.compile_regex(&src, &flags)?;
    Ok(Value::Object(alloc(proto, src, flags, prog)))
}

/// O objeto RegExp com `lastIndex` = 0.
fn alloc(proto: ObjRef, source: JsStr, flags: JsStr, prog: Rc<Program>) -> ObjRef {
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
    o
}

/// RegExp.escape: escapa o que teria sentido num padrão (EncodeForRegExpEscape).
fn escape(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::String(s) = a.arg(0) else {
        return Err(vm.type_error("RegExp.escape requires a string"));
    };
    let u = s.units();
    let mut out: Vec<u16> = Vec::with_capacity(u.len());
    let push = |out: &mut Vec<u16>, t: &str| out.extend(t.encode_utf16());
    let mut i = 0;
    while i < u.len() {
        // ponto de código (par substituto junto; substituto solto fica sozinho)
        let (c, n) = match (u[i], u.get(i + 1)) {
            (hi @ 0xD800..=0xDBFF, Some(&lo @ 0xDC00..=0xDFFF)) => (0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00), 2),
            (x, _) => (x as u32, 1),
        };
        let ch = char::from_u32(c);
        if i == 0 && ch.is_some_and(|c| c.is_ascii_alphanumeric()) {
            push(&mut out, &format!("\\x{c:02x}"));
        } else if ch.is_some_and(|c| "^$\\.*+?()[]{}|/".contains(c)) {
            out.extend_from_slice(&[0x5C, c as u16]);
        } else if let Some(e) = match c {
            0x09 => Some('t'),
            0x0A => Some('n'),
            0x0B => Some('v'),
            0x0C => Some('f'),
            0x0D => Some('r'),
            _ => None,
        } {
            out.extend_from_slice(&[0x5C, e as u16]);
        } else if ch.is_some_and(|c| ",-=<>#&!%:;@~'`\"".contains(c)) || crate::regex::is_space(c) || (0xD800..0xE000).contains(&c) {
            if c <= 0xFF {
                push(&mut out, &format!("\\x{c:02x}"));
            } else {
                for k in 0..n {
                    push(&mut out, &format!("\\u{:04x}", u[i + k]));
                }
            }
        } else {
            out.extend_from_slice(&u[i..i + n]);
        }
        i += n;
    }
    Ok(Value::String(JsStr::from_units(out)))
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
            match c {
                0x0A => out.push('n' as u16),
                0x0D => out.push('r' as u16),
                0x2028 => out.extend("u2028".encode_utf16()),
                0x2029 => out.extend("u2029".encode_utf16()),
                _ => out.push(c),
            }
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
    let s = vm.to_string(&s)?;
    let f = vm.get_str(&a.this, "flags")?;
    let f = vm.to_string(&f)?;
    let mut out: Vec<u16> = Vec::with_capacity(s.len() + f.len() + 2);
    out.push(0x2F);
    out.extend_from_slice(s.units());
    out.push(0x2F);
    out.extend_from_slice(f.units());
    Ok(Value::String(JsStr::from_units(out)))
}

fn compile_legacy(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    // só RegExp do próprio construtor (sem [[LegacyFeaturesEnabled]] nas subclasses)
    let legacy = match &a.this {
        Value::Object(o) => {
            let b = o.borrow();
            matches!(b.kind, Kind::RegExp { .. }) && b.proto.as_ref().is_some_and(|p| p.ptr_eq(&vm.realm.regexp_proto))
        }
        _ => false,
    };
    let (Value::Object(o), true) = (a.this.clone(), legacy) else {
        return Err(vm.type_error("RegExp.prototype.compile called on incompatible receiver"));
    };
    let (src, fl) = match regexp_parts(&a.arg(0)) {
        Some((s, f, _)) => {
            if !a.arg(1).is_undefined() {
                return Err(vm.type_error("Cannot supply flags when constructing one RegExp from another"));
            }
            (s, f)
        }
        None => {
            let s = if a.arg(0).is_undefined() { JsStr::empty() } else { vm.to_string(&a.arg(0))? };
            let f = if a.arg(1).is_undefined() { JsStr::empty() } else { vm.to_string(&a.arg(1))? };
            (s, f)
        }
    };
    let prog = vm.compile_regex(&src, &fl)?;
    o.borrow_mut().kind = Kind::RegExp { source: src, flags: fl, prog };
    vm.put(&a.this, PropKey::str("lastIndex"), Value::Number(0.0), true)?;
    Ok(a.this.clone())
}

// --------------------------------------------------------------- execução

fn last_index(vm: &mut Vm, r: &Value) -> Result<usize, Value> {
    // caminho rápido: propriedade própria numérica
    if let Value::Object(o) = r
        && let Some(Property {
            slot: Slot::Data { value: Value::Number(n), .. },
            ..
        }) = o.borrow().props.get(&PropKey::str("lastIndex"))
    {
        return Ok(if *n <= 0.0 || n.is_nan() { 0 } else { n.min(9007199254740991.0) as usize });
    }
    let v = vm.get_str(r, "lastIndex")?;
    let n = vm.to_integer(&v)?;
    Ok(if n <= 0.0 { 0 } else { n.min(9007199254740991.0) as usize })
}

fn set_last_index(vm: &mut Vm, r: &Value, v: usize) -> Result<(), Value> {
    vm.put(r, PropKey::str("lastIndex"), Value::Number(v as f64), true)
}

/// Uma ocorrência: as capturas e o programa que casou.
struct Hit {
    caps: Vec<usize>,
    prog: Rc<Program>,
}

/// RegExpBuiltinExec sem montar o objeto de resultado.
fn builtin_match(vm: &mut Vm, r: &Value, s: &JsStr) -> Result<Option<Hit>, Value> {
    // lastIndex primeiro: o valueOf dele pode recompilar a regex
    let mut li = last_index(vm, r)?;
    let Some((_, _, prog)) = regexp_parts(r) else {
        return Err(vm.type_error("RegExp exec method called on incompatible receiver"));
    };
    let (global, sticky) = (prog.flags.global, prog.flags.sticky);
    if !global && !sticky {
        li = 0;
    }
    if li > s.len() {
        if global || sticky {
            set_last_index(vm, r, 0)?;
        }
        return Ok(None);
    }
    match prog.search(s.units(), li, sticky) {
        None => {
            if global || sticky {
                set_last_index(vm, r, 0)?;
            }
            Ok(None)
        }
        Some(caps) => {
            if global || sticky {
                set_last_index(vm, r, caps[1])?;
            }
            vm.regexp_statics = (s.clone(), caps.clone());
            Ok(Some(Hit { caps, prog }))
        }
    }
}

fn slice_or_undef(s: &JsStr, a: usize, b: usize) -> Value {
    if a == NONE || b == NONE { Value::Undefined } else { Value::String(s.slice(a, b)) }
}

/// Monta o array de resultado de exec (com index, input, groups, indices).
fn hit_to_array(vm: &mut Vm, prog: &Program, s: &JsStr, hit: &Hit) -> Value {
    let n = prog.ngroups as usize + 1;
    let items: Vec<Value> = (0..n).map(|g| slice_or_undef(s, hit.caps[2 * g], hit.caps[2 * g + 1])).collect();
    let arr = vm.new_array(items.clone());
    vm.define_data(&arr, PropKey::str("index"), Value::Number(hit.caps[0] as f64));
    vm.define_data(&arr, PropKey::str("input"), Value::String(s.clone()));
    let groups = if prog.names.is_empty() {
        Value::Undefined
    } else {
        let g = ObjRef::new(Object::new(None, Kind::Ordinary));
        for (k, (name, _)) in prog.names.iter().enumerate() {
            let idx = prog.named_group(k, &hit.caps);
            vm.define_data(&g, PropKey::Str(JsStr::new(name)), items[idx as usize].clone());
        }
        Value::Object(g)
    };
    vm.define_data(&arr, PropKey::str("groups"), groups);
    if prog.flags.has_indices {
        let pairs: Vec<Value> = (0..n)
            .map(|g| {
                let (a, b) = (hit.caps[2 * g], hit.caps[2 * g + 1]);
                if a == NONE || b == NONE {
                    Value::Undefined
                } else {
                    vm.array_value(vec![Value::Number(a as f64), Value::Number(b as f64)])
                }
            })
            .collect();
        let ind = vm.new_array(pairs.clone());
        let ig = if prog.names.is_empty() {
            Value::Undefined
        } else {
            let g = ObjRef::new(Object::new(None, Kind::Ordinary));
            for (k, (name, _)) in prog.names.iter().enumerate() {
                let idx = prog.named_group(k, &hit.caps);
                vm.define_data(&g, PropKey::Str(JsStr::new(name)), pairs[idx as usize].clone());
            }
            Value::Object(g)
        };
        vm.define_data(&ind, PropKey::str("groups"), ig);
        vm.define_data(&arr, PropKey::str("indices"), Value::Object(ind));
    }
    Value::Object(arr)
}

pub fn exec(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    this_parts(vm, a, "exec")?;
    let s = vm.to_string(&a.arg(0))?;
    match builtin_match(vm, &a.this, &s)? {
        Some(hit) => Ok(hit_to_array(vm, &hit.prog, &s, &hit)),
        None => Ok(Value::Null),
    }
}

/// O `exec` de R é o original? (dá para pular a criação dos arrays)
fn builtin_exec_of(vm: &mut Vm, r: &Value) -> Result<Option<Rc<Program>>, Value> {
    let Some((_, _, prog)) = regexp_parts(r) else { return Ok(None) };
    let e = vm.get_str(r, "exec")?;
    Ok(match &e {
        Value::Object(f) if f.ptr_eq(&vm.realm.regexp_exec) => Some(prog),
        _ => None,
    })
}

/// RegExpExec: chama `exec` (que pode ter sido trocado pelo usuário).
fn regexp_exec(vm: &mut Vm, r: &Value, s: &JsStr) -> Result<Value, Value> {
    let e = vm.get_str(r, "exec")?;
    if e.is_callable() {
        let res = vm.call(&e, r.clone(), vec![Value::String(s.clone())])?;
        if !matches!(res, Value::Object(_) | Value::Null) {
            return Err(vm.type_error("object or null expected from exec"));
        }
        return Ok(res);
    }
    match builtin_match(vm, r, s)? {
        Some(hit) => Ok(hit_to_array(vm, &hit.prog, s, &hit)),
        None => Ok(Value::Null),
    }
}

fn advance(s: &JsStr, i: usize, unicode: bool) -> usize {
    let u = s.units();
    if unicode && i + 1 < u.len() && (0xD800..0xDC00).contains(&u[i]) && (0xDC00..0xE000).contains(&u[i + 1]) {
        i + 2
    } else {
        i + 1
    }
}

fn test(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if !matches!(a.this, Value::Object(_)) {
        return Err(vm.type_error("RegExp.prototype.test called on incompatible receiver"));
    }
    let s = vm.to_string(&a.arg(0))?;
    if builtin_exec_of(vm, &a.this)?.is_some() {
        return Ok(Value::Bool(builtin_match(vm, &a.this, &s)?.is_some()));
    }
    Ok(Value::Bool(!matches!(regexp_exec(vm, &a.this, &s)?, Value::Null)))
}

/// O acessor `name` de `o` é o getter nativo `f` (o original)?
fn native_getter(o: &ObjRef, name: &PropKey, f: NativeFn) -> bool {
    match o.borrow().props.get(name) {
        Some(Property {
            slot: Slot::Accessor { get: Some(g), .. },
            ..
        }) => is_native(g, f),
        _ => false,
    }
}

/// A propriedade de dados `name` de `o` é a função nativa `f`? Devolve a função.
fn native_value(o: &ObjRef, name: &PropKey, f: NativeFn) -> Option<ObjRef> {
    match o.borrow().props.get(name) {
        Some(Property {
            slot: Slot::Data { value: Value::Object(v), .. },
            ..
        }) if is_native(v, f) => Some(v.clone()),
        _ => None,
    }
}

fn is_native(o: &ObjRef, f: NativeFn) -> bool {
    matches!(&o.borrow().kind, Kind::Function(Func::Native(n)) if std::ptr::fn_addr_eq(n.f, f))
}

/// Um RegExp sem propriedades próprias além de `lastIndex`, com o
/// RegExp.prototype e o RegExp intactos (constructor, Symbol.species, exec,
/// Symbol.match, flags e os getters de cada flag): então SpeciesConstructor,
/// Get("flags") e Construct não têm efeito observável e dá para pular direto.
/// Devolve (source, flags) do próprio objeto.
fn pristine_rx(vm: &Vm, r: &Value) -> Option<(JsStr, JsStr)> {
    let Value::Object(o) = r else { return None };
    let b = o.borrow();
    let Kind::RegExp { source, flags, .. } = &b.kind else { return None };
    if b.props.len() != 1 {
        return None;
    }
    let p = b.proto.as_ref().filter(|p| p.ptr_eq(&vm.realm.regexp_proto))?;
    thread_local! {
        /// As chaves conferidas (sem alocar a cada chamada).
        static CHAVES: [PropKey; 11] = ["constructor", "exec", "flags", "hasIndices", "global", "ignoreCase", "multiline", "dotAll", "unicode", "unicodeSets", "sticky"].map(PropKey::str);
    }
    let getters: [NativeFn; 9] = [self::flags, flag_d, flag_g, flag_i, flag_m, flag_s, flag_u, flag_v, flag_y];
    let ok = CHAVES.with(|k| {
        let Some(ctor) = native_value(p, &k[0], regexp_ctor) else { return false };
        let exec_ok = matches!(p.borrow().props.get(&k[1]), Some(Property {
            slot: Slot::Data { value: Value::Object(f), .. },
            ..
        }) if f.ptr_eq(&vm.realm.regexp_exec));
        exec_ok
            && native_value(p, &PropKey::Sym(vm.realm.sym_match.clone()), sym_match).is_some()
            && getters.iter().zip(&k[2..]).all(|(&f, n)| native_getter(p, n, f))
            && native_getter(&ctor, &PropKey::Sym(vm.realm.sym_species.clone()), super::array::return_this)
    });
    ok.then(|| (source.clone(), flags.clone()))
}

fn rx_flags(vm: &mut Vm, r: &Value) -> Result<String, Value> {
    if let Some((_, f)) = pristine_rx(vm, r) {
        return Ok(f.to_rust());
    }
    let f = vm.get_str(r, "flags")?;
    vm.to_rust_string(&f)
}

fn sym_match(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let r = a.this.clone();
    let s = vm.to_string(&a.arg(0))?;
    let fl = rx_flags(vm, &r)?;
    if !fl.contains('g') {
        return regexp_exec(vm, &r, &s);
    }
    let unicode = fl.contains('u') || fl.contains('v');
    set_last_index(vm, &r, 0)?;
    let mut out = Vec::new();
    let fast = builtin_exec_of(vm, &r)?;
    if let Some(prog) = fast.as_ref().filter(|p| !p.flags.sticky) {
        // caminho rápido: percorre as ocorrências direto, sem passar por lastIndex
        let mut pos = 0usize;
        while pos <= s.len() {
            let Some(caps) = prog.search(s.units(), pos, false) else { break };
            out.push(Value::String(s.slice(caps[0], caps[1])));
            pos = if caps[1] == caps[0] { advance(&s, caps[1], unicode) } else { caps[1] };
        }
        set_last_index(vm, &r, 0)?;
    } else if fast.is_some() {
        while let Some(hit) = builtin_match(vm, &r, &s)? {
            out.push(Value::String(s.slice(hit.caps[0], hit.caps[1])));
            if hit.caps[0] == hit.caps[1] {
                let li = last_index(vm, &r)?;
                set_last_index(vm, &r, advance(&s, li, unicode))?;
            }
        }
    } else {
        loop {
            let res = regexp_exec(vm, &r, &s)?;
            if matches!(res, Value::Null) {
                break;
            }
            let m0 = vm.get_index(&res, &Value::Number(0.0))?;
            let ms = vm.to_string(&m0)?;
            if ms.is_empty() {
                let li = last_index(vm, &r)?;
                set_last_index(vm, &r, advance(&s, li, unicode))?;
            }
            out.push(Value::String(ms));
        }
    }
    Ok(if out.is_empty() { Value::Null } else { vm.array_value(out) })
}

fn sym_search(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let r = a.this.clone();
    let s = vm.to_string(&a.arg(0))?;
    let prev = vm.get_str(&r, "lastIndex")?;
    if !same_value(&prev, &Value::Number(0.0)) {
        set_last_index(vm, &r, 0)?;
    }
    let res = regexp_exec(vm, &r, &s)?;
    let cur = vm.get_str(&r, "lastIndex")?;
    if !same_value(&cur, &prev) {
        vm.put(&r, PropKey::str("lastIndex"), prev, true)?;
    }
    if matches!(res, Value::Null) {
        return Ok(Value::Number(-1.0));
    }
    vm.get_str(&res, "index")
}

fn sym_replace(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let r = a.this.clone();
    if !matches!(r, Value::Object(_)) {
        return Err(vm.type_error("RegExp.prototype[Symbol.replace] called on non-object"));
    }
    let s = vm.to_string(&a.arg(0))?;
    let replace_value = a.arg(1);
    let functional = replace_value.is_callable();
    let template = if functional { JsStr::empty() } else { vm.to_string(&replace_value)? };
    let fl = rx_flags(vm, &r)?;
    let global = fl.contains('g');
    let unicode = fl.contains('u') || fl.contains('v');
    if global {
        set_last_index(vm, &r, 0)?;
    }
    // (casado, posição, capturas, grupos)
    let mut results: Vec<(JsStr, usize, Vec<Value>, Value)> = Vec::new();
    let fast = builtin_exec_of(vm, &r)?;
    if let Some(prog) = fast.as_ref().filter(|p| global && !p.flags.sticky) {
        // caminho rápido: percorre as ocorrências direto, sem passar por lastIndex
        let mut pos = 0usize;
        while pos <= s.len() {
            let Some(caps) = prog.search(s.units(), pos, false) else { break };
            let matched = s.slice(caps[0], caps[1]);
            let cap_vals: Vec<Value> = (1..=prog.ngroups as usize).map(|g| slice_or_undef(&s, caps[2 * g], caps[2 * g + 1])).collect();
            let groups = if prog.names.is_empty() {
                Value::Undefined
            } else {
                let g = ObjRef::new(Object::new(None, Kind::Ordinary));
                for (k, (name, _)) in prog.names.iter().enumerate() {
                    let idx = prog.named_group(k, &caps);
                    vm.define_data(&g, PropKey::Str(JsStr::new(name)), cap_vals[idx as usize - 1].clone());
                }
                Value::Object(g)
            };
            pos = if caps[1] == caps[0] { advance(&s, caps[1], unicode) } else { caps[1] };
            results.push((matched, caps[0], cap_vals, groups));
        }
        set_last_index(vm, &r, 0)?;
    } else if fast.is_some() {
        while let Some(hit) = builtin_match(vm, &r, &s)? {
            let prog = &hit.prog;
            let matched = s.slice(hit.caps[0], hit.caps[1]);
            let caps: Vec<Value> = (1..=prog.ngroups as usize).map(|g| slice_or_undef(&s, hit.caps[2 * g], hit.caps[2 * g + 1])).collect();
            let groups = if prog.names.is_empty() {
                Value::Undefined
            } else {
                let g = ObjRef::new(Object::new(None, Kind::Ordinary));
                for (k, (name, _)) in prog.names.iter().enumerate() {
                    let idx = prog.named_group(k, &hit.caps);
                    vm.define_data(&g, PropKey::Str(JsStr::new(name)), caps[idx as usize - 1].clone());
                }
                Value::Object(g)
            };
            let empty = matched.is_empty();
            results.push((matched, hit.caps[0], caps, groups));
            if !global {
                break;
            }
            if empty {
                let li = last_index(vm, &r)?;
                set_last_index(vm, &r, advance(&s, li, unicode))?;
            }
        }
    } else {
        // como o spec: primeiro todos os exec (lendo só o "0"), depois os resultados
        let mut objs = Vec::new();
        loop {
            let res = regexp_exec(vm, &r, &s)?;
            if matches!(res, Value::Null) {
                break;
            }
            objs.push(res.clone());
            if !global {
                break;
            }
            let m0 = vm.get_index(&res, &Value::Number(0.0))?;
            if vm.to_string(&m0)?.is_empty() {
                let li = last_index(vm, &r)?;
                set_last_index(vm, &r, advance(&s, li, unicode))?;
            }
        }
        let mut out: Vec<u16> = Vec::with_capacity(s.len());
        let mut next = 0usize;
        for res in objs {
            let n = match &res {
                Value::Object(o) => vm.length_of(o)?,
                _ => 0,
            };
            let m0 = vm.get_index(&res, &Value::Number(0.0))?;
            let matched = vm.to_string(&m0)?;
            let pos_v = vm.get_str(&res, "index")?;
            let pos = vm.to_integer(&pos_v)?.clamp(0.0, s.len() as f64) as usize;
            let mut caps = Vec::new();
            for k in 1..n {
                let c = vm.get_index(&res, &Value::Number(k as f64))?;
                caps.push(if c.is_undefined() { c } else { Value::String(vm.to_string(&c)?) });
            }
            let groups = vm.get_str(&res, "groups")?;
            let repl = replacement(vm, &replace_value, &template, &matched, &s, pos, &caps, &groups)?;
            if pos >= next {
                out.extend_from_slice(&s.units()[next..pos]);
                out.extend_from_slice(repl.units());
                next = (pos + matched.len()).min(s.len());
            }
        }
        out.extend_from_slice(&s.units()[next.min(s.len())..]);
        return Ok(Value::String(JsStr::from_units(out)));
    }
    let mut out: Vec<u16> = Vec::with_capacity(s.len());
    let mut next = 0usize;
    for (matched, pos, caps, groups) in results {
        let pos = pos.min(s.len());
        let repl = replacement(vm, &replace_value, &template, &matched, &s, pos, &caps, &groups)?;
        if pos >= next {
            out.extend_from_slice(&s.units()[next..pos]);
            out.extend_from_slice(repl.units());
            next = (pos + matched.len()).min(s.len());
        }
    }
    out.extend_from_slice(&s.units()[next.min(s.len())..]);
    Ok(Value::String(JsStr::from_units(out)))
}

/// O texto que substitui uma ocorrência (função ou modelo com `$1`, `$<nome>`...).
#[allow(clippy::too_many_arguments)]
fn replacement(
    vm: &mut Vm,
    replace_value: &Value,
    template: &JsStr,
    matched: &JsStr,
    s: &JsStr,
    pos: usize,
    caps: &[Value],
    groups: &Value,
) -> Result<JsStr, Value> {
    if replace_value.is_callable() {
        let mut args = vec![Value::String(matched.clone())];
        args.extend(caps.iter().cloned());
        args.push(Value::Number(pos as f64));
        args.push(Value::String(s.clone()));
        if !groups.is_undefined() {
            args.push(groups.clone());
        }
        let rv = vm.call(replace_value, Value::Undefined, args)?;
        return vm.to_string(&rv);
    }
    let groups = if groups.is_undefined() { groups.clone() } else { Value::Object(vm.to_object(groups)?) };
    get_substitution(vm, matched, s, pos, caps, &groups, template)
}

/// SpeciesConstructor(rx, %RegExp%) e se deu o próprio %RegExp% (guardado
/// no dado da função nativa).
fn species_ctor(vm: &mut Vm, a: &CallArgs, rx: &ObjRef) -> Result<(ObjRef, bool), Value> {
    let Value::Object(def) = super::native_data(&a.callee) else {
        return Err(vm.type_error("RegExp species constructor not available"));
    };
    let c = match vm.get_obj(rx, &PropKey::str("constructor"))? {
        Value::Undefined => return Ok((def, true)),
        Value::Object(c) => c,
        _ => return Err(vm.type_error("object.constructor is not an object")),
    };
    let species = vm.realm.sym_species.clone();
    match vm.get_obj(&c, &PropKey::Sym(species))? {
        Value::Undefined | Value::Null => Ok((def, true)),
        Value::Object(sp) if sp.is_constructor() => {
            let d = sp.ptr_eq(&def);
            Ok((sp, d))
        }
        _ => Err(vm.type_error("object.constructor[Symbol.species] is not a constructor")),
    }
}

/// Um RegExp recém-criado pelo %RegExp% com o `exec` original? Então dá para
/// casar direto, sem passar por `lastIndex` (nada disso é observável).
fn pristine(vm: &Vm, v: &Value) -> Option<Rc<Program>> {
    let Value::Object(o) = v else { return None };
    let b = o.borrow();
    let Kind::RegExp { prog, .. } = &b.kind else { return None };
    if b.props.len() != 1 || !b.proto.as_ref().is_some_and(|p| p.ptr_eq(&vm.realm.regexp_proto)) || !exec_original(vm) {
        return None;
    }
    Some(prog.clone())
}

/// O RegExp.prototype.exec ainda é o original?
fn exec_original(vm: &Vm) -> bool {
    thread_local! {
        static EXEC: PropKey = PropKey::str("exec");
    }
    EXEC.with(|k| {
        matches!(vm.realm.regexp_proto.borrow().props.get(k), Some(Property {
            slot: Slot::Data { value: Value::Object(f), .. },
            ..
        }) if f.ptr_eq(&vm.realm.regexp_exec))
    })
}

/// ToString(Get(rx, "flags")).
fn get_flags(vm: &mut Vm, r: &Value) -> Result<JsStr, Value> {
    let f = vm.get_str(r, "flags")?;
    vm.to_string(&f)
}

fn has_unit(s: &JsStr, c: char) -> bool {
    s.units().contains(&(c as u16))
}

fn sym_split(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let r = a.this.clone();
    let Value::Object(ro) = &r else {
        return Err(vm.type_error("RegExp.prototype[Symbol.split] called on non-object"));
    };
    let s = vm.to_string(&a.arg(0))?;
    // RegExp intacto: SpeciesConstructor, flags e Construct não têm efeito
    // observável; o splitter (uma cópia com "y") só é criado se precisar
    let intacto = pristine_rx(vm, &r).and_then(|(src, fl)| regexp_parts(&r).map(|(_, _, p)| (src, fl, p)));
    let (mut splitter, unicode, default) = match &intacto {
        Some((_, _, prog)) => (Value::Undefined, prog.flags.full_unicode(), true),
        None => {
            let (c, default) = species_ctor(vm, a, ro)?;
            let flags = get_flags(vm, &r)?;
            let unicode = has_unit(&flags, 'u') || has_unit(&flags, 'v');
            let mut nf = flags.units().to_vec();
            if !has_unit(&flags, 'y') {
                nf.push('y' as u16);
            }
            let splitter = vm.construct(&c, vec![r.clone(), Value::String(JsStr::from_units(nf))], &c)?;
            (splitter, unicode, default)
        }
    };
    // o limite pode rodar código (valueOf) que troca o exec
    let lim = if a.arg(1).is_undefined() { u32::MAX } else { crate::conv::to_uint32(vm.to_number(&a.arg(1))?) };
    let mut out: Vec<Value> = Vec::new();
    if lim == 0 {
        return Ok(vm.array_value(out));
    }
    let size = s.len();
    let rapido = match intacto {
        // o caminho rápido procura sem depender do "y": serve o próprio programa
        Some((_, _, prog)) if exec_original(vm) => Some(prog),
        Some((src, flags, _)) => {
            let mut nf = flags.units().to_vec();
            if !has_unit(&flags, 'y') {
                nf.push('y' as u16);
            }
            let nf = JsStr::from_units(nf);
            let prog = vm.compile_regex(&src, &nf)?;
            splitter = Value::Object(alloc(vm.realm.regexp_proto.clone(), src, nf, prog));
            None
        }
        None => pristine(vm, &splitter).filter(|_| default),
    };
    if let Some(prog) = rapido {
        // caminho rápido: procura as ocorrências direto
        if size == 0 {
            if prog.match_at(s.units(), 0).is_none() {
                out.push(Value::String(s));
            }
            return Ok(vm.array_value(out));
        }
        let mut p = 0usize;
        let mut q = 0usize;
        while q < size {
            let Some(caps) = prog.search(s.units(), q, false) else { break };
            let (start, end) = (caps[0], caps[1].min(size));
            if start >= size {
                break;
            }
            if end == p {
                q = advance(&s, start, unicode);
                continue;
            }
            out.push(Value::String(s.slice(p, start)));
            if out.len() as u32 >= lim {
                return Ok(vm.array_value(out));
            }
            p = end;
            for g in 1..=prog.ngroups as usize {
                out.push(slice_or_undef(&s, caps[2 * g], caps[2 * g + 1]));
                if out.len() as u32 >= lim {
                    return Ok(vm.array_value(out));
                }
            }
            q = p;
        }
        out.push(Value::String(s.slice(p, size)));
        return Ok(vm.array_value(out));
    }
    // o algoritmo do spec, passo a passo (exec e lastIndex observáveis)
    if size == 0 {
        if matches!(regexp_exec(vm, &splitter, &s)?, Value::Null) {
            out.push(Value::String(s));
        }
        return Ok(vm.array_value(out));
    }
    let mut p = 0usize;
    let mut q = 0usize;
    while q < size {
        set_last_index(vm, &splitter, q)?;
        let z = regexp_exec(vm, &splitter, &s)?;
        if matches!(z, Value::Null) {
            q = advance(&s, q, unicode);
            continue;
        }
        let e = {
            let li = vm.get_str(&splitter, "lastIndex")?;
            let n = vm.to_integer(&li)?;
            (if n <= 0.0 { 0.0 } else { n.min(size as f64) }) as usize
        };
        if e == p {
            q = advance(&s, q, unicode);
            continue;
        }
        out.push(Value::String(s.slice(p, q)));
        if out.len() as u32 >= lim {
            return Ok(vm.array_value(out));
        }
        p = e;
        let n = match &z {
            Value::Object(zo) => vm.length_of(zo)?,
            _ => 0,
        };
        for i in 1..n {
            out.push(vm.get_index(&z, &Value::Number(i as f64))?);
            if out.len() as u32 >= lim {
                return Ok(vm.array_value(out));
            }
        }
        q = p;
    }
    out.push(Value::String(s.slice(p, size)));
    Ok(vm.array_value(out))
}

fn sym_match_all(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let r = a.this.clone();
    let Value::Object(ro) = &r else {
        return Err(vm.type_error("RegExp.prototype[Symbol.matchAll] called on non-object"));
    };
    let s = vm.to_string(&a.arg(0))?;
    let (matcher, flags) = match pristine_rx(vm, &r) {
        Some((src, flags)) => {
            let prog = vm.compile_regex(&src, &flags)?;
            let m = alloc(vm.realm.regexp_proto.clone(), src, flags.clone(), prog);
            (Value::Object(m), flags)
        }
        None => {
            let (c, _) = species_ctor(vm, a, ro)?;
            let flags = get_flags(vm, &r)?;
            let m = vm.construct(&c, vec![r.clone(), Value::String(flags.clone())], &c)?;
            (m, flags)
        }
    };
    let li = {
        let v = vm.get_str(&r, "lastIndex")?;
        vm.to_integer(&v)?.clamp(0.0, 9007199254740991.0)
    };
    vm.put(&matcher, PropKey::str("lastIndex"), Value::Number(li), true)?;
    let Value::Object(m) = matcher else {
        return Err(vm.type_error("RegExp species constructor returned a non-object"));
    };
    let it = ObjRef::new(Object::new(
        Some(vm.realm.regexp_string_iter_proto.clone()),
        Kind::RegExpStringIter {
            rx: m,
            s,
            global: has_unit(&flags, 'g'),
            unicode: has_unit(&flags, 'u') || has_unit(&flags, 'v'),
            done: false,
        },
    ));
    Ok(Value::Object(it))
}

fn string_iter_next(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(it) = &a.this else {
        return Err(vm.type_error("next method called on incompatible receiver"));
    };
    let (rx, s, global, unicode, done) = match &it.borrow().kind {
        Kind::RegExpStringIter { rx, s, global, unicode, done } => (rx.clone(), s.clone(), *global, *unicode, *done),
        _ => return Err(vm.type_error("next method called on incompatible receiver")),
    };
    let set_done = |it: &ObjRef| {
        if let Kind::RegExpStringIter { done, .. } = &mut it.borrow_mut().kind {
            *done = true;
        }
    };
    if done {
        return Ok(iter_result(vm, Value::Undefined, true));
    }
    let rv = Value::Object(rx);
    let res = regexp_exec(vm, &rv, &s)?;
    if matches!(res, Value::Null) {
        set_done(it);
        return Ok(iter_result(vm, Value::Undefined, true));
    }
    if !global {
        set_done(it);
        return Ok(iter_result(vm, res, false));
    }
    let m0 = vm.get_index(&res, &Value::Number(0.0))?;
    if vm.to_string(&m0)?.is_empty() {
        let li = last_index(vm, &rv)?;
        set_last_index(vm, &rv, advance(&s, li, unicode))?;
    }
    Ok(iter_result(vm, res, false))
}
