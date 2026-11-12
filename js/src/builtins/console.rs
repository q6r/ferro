//! console e a formatação de valores no estilo do Node (`inspect`).

use super::Realm;
use crate::conv::number_to_string;
use crate::ops::data_in_chain;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Out, Vm};

pub fn install(r: &Realm) {
    let c = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    for (name, f) in [
        ("log", log as NativeFn),
        ("info", log),
        ("debug", log),
        ("warn", warn),
        ("error", warn),
        ("trace", trace),
        ("dir", dir),
        ("dirxml", log),
        ("table", log),
        ("assert", assert),
        ("time", time),
        ("timeEnd", time_end),
        ("timeLog", time_log),
        ("count", count),
        ("countReset", count_reset),
        ("group", group),
        ("groupCollapsed", group),
        ("groupEnd", group_end),
        ("clear", clear),
    ] {
        r.method(&c, name, 0, f);
    }
    r.global_value("console", Value::Object(c));
}

fn emit(vm: &mut Vm, kind: Out, text: &str) {
    let pad = " ".repeat(vm.console_indent);
    let text = if pad.is_empty() {
        text.to_string()
    } else {
        text.lines().map(|l| format!("{pad}{l}")).collect::<Vec<_>>().join("\n")
    };
    (vm.output)(kind, &text);
}

/// Formata os argumentos como o console.log do Node (com %s, %d, %o...).
pub fn format_args(vm: &mut Vm, args: &[Value]) -> Result<String, Value> {
    let mut out = String::new();
    let mut rest = 0;
    if let Some(Value::String(fmt)) = args.first() {
        let f = fmt.to_rust();
        let mut it = f.chars().peekable();
        let mut next = 1;
        while let Some(c) = it.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            let Some(&d) = it.peek() else {
                out.push('%');
                break;
            };
            match d {
                's' | 'd' | 'i' | 'f' | 'o' | 'O' | 'j' | 'c' if next < args.len() => {
                    it.next();
                    let a = args[next].clone();
                    next += 1;
                    match d {
                        's' => match &a {
                            Value::String(s) => out.push_str(&s.to_rust()),
                            Value::Object(_) => out.push_str(&inspect(vm, &a, 1)),
                            _ => out.push_str(&inspect(vm, &a, 2)),
                        },
                        'd' | 'i' => {
                            if matches!(a, Value::Object(_)) {
                                out.push_str("NaN");
                            } else {
                                let n = vm.to_number(&a)?;
                                let n = if d == 'i' { n.trunc() } else { n };
                                out.push_str(&number_to_string(n));
                            }
                        }
                        'f' => {
                            let s = vm.to_string(&a)?;
                            out.push_str(&number_to_string(crate::conv::parse_float(s.units())));
                        }
                        'j' => {
                            let json = vm.get_global(&JsStr::new("JSON"))?;
                            let f = vm.get_str(&json, "stringify")?;
                            let s = vm.call(&f, json, vec![a])?;
                            out.push_str(&vm.to_rust_string(&s)?);
                        }
                        'c' => {}
                        _ => out.push_str(&inspect(vm, &a, 4)),
                    }
                }
                '%' => {
                    it.next();
                    out.push('%');
                }
                _ => out.push('%'),
            }
        }
        rest = next;
    }
    for a in args.iter().skip(rest) {
        if !out.is_empty() || rest > 0 {
            out.push(' ');
        }
        match a {
            Value::String(s) => out.push_str(&s.to_rust()),
            _ => out.push_str(&inspect(vm, a, 2)),
        }
    }
    Ok(out)
}

fn log(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = format_args(vm, &a.args)?;
    emit(vm, Out::Log, &s);
    Ok(Value::Undefined)
}

fn warn(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = format_args(vm, &a.args)?;
    emit(vm, Out::Error, &s);
    Ok(Value::Undefined)
}

fn trace(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = format_args(vm, &a.args)?;
    let header = if s.is_empty() { "Trace".to_string() } else { format!("Trace: {s}") };
    let st = vm.stack_trace(&header);
    emit(vm, Out::Error, &st);
    Ok(Value::Undefined)
}

fn dir(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = inspect(vm, &a.arg(0), 2);
    emit(vm, Out::Log, &s);
    Ok(Value::Undefined)
}

fn assert(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if crate::vm::truthy(&a.arg(0)) {
        return Ok(Value::Undefined);
    }
    let rest: Vec<Value> = a.args.iter().skip(1).cloned().collect();
    let s = format_args(vm, &rest)?;
    let msg = if s.is_empty() { "Assertion failed".to_string() } else { format!("Assertion failed: {s}") };
    emit(vm, Out::Error, &msg);
    Ok(Value::Undefined)
}

fn label(vm: &mut Vm, a: &CallArgs) -> Result<String, Value> {
    if a.arg(0).is_undefined() { Ok("default".into()) } else { vm.to_rust_string(&a.arg(0)) }
}

fn time(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let l = label(vm, a)?;
    vm.console_timers.insert(l, std::time::Instant::now());
    Ok(Value::Undefined)
}

fn elapsed(vm: &mut Vm, l: &str) -> Option<String> {
    vm.console_timers.get(l).map(|t| {
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms >= 1000.0 { format!("{l}: {:.3}s", ms / 1000.0) } else { format!("{l}: {ms:.3}ms") }
    })
}

fn time_end(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let l = label(vm, a)?;
    if let Some(s) = elapsed(vm, &l) {
        emit(vm, Out::Log, &s);
        vm.console_timers.remove(&l);
    }
    Ok(Value::Undefined)
}

fn time_log(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let l = label(vm, a)?;
    if let Some(s) = elapsed(vm, &l) {
        let rest: Vec<Value> = a.args.iter().skip(1).cloned().collect();
        let extra = format_args(vm, &rest)?;
        let s = if extra.is_empty() { s } else { format!("{s} {extra}") };
        emit(vm, Out::Log, &s);
    }
    Ok(Value::Undefined)
}

fn count(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let l = label(vm, a)?;
    let n = {
        let c = vm.console_counts.entry(l.clone()).or_insert(0);
        *c += 1;
        *c
    };
    emit(vm, Out::Log, &format!("{l}: {n}"));
    Ok(Value::Undefined)
}

fn count_reset(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let l = label(vm, a)?;
    vm.console_counts.insert(l, 0);
    Ok(Value::Undefined)
}

fn group(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if !a.args.is_empty() {
        let s = format_args(vm, &a.args)?;
        emit(vm, Out::Log, &s);
    }
    vm.console_indent += 2;
    Ok(Value::Undefined)
}

fn group_end(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    vm.console_indent = vm.console_indent.saturating_sub(2);
    Ok(Value::Undefined)
}

fn clear(_vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------- inspect

/// Representação legível de um valor (como `util.inspect` do Node).
pub fn inspect(_vm: &Vm, v: &Value, depth: usize) -> String {
    let mut ins = Inspector {
        seen: Vec::new(),
        max_depth: depth,
        refs: Vec::new(),
    };
    ins.fmt(v, 0, 0, false).0
}

struct Inspector {
    seen: Vec<usize>,
    max_depth: usize,
    /// Objetos referenciados circularmente (para "<ref *1>").
    refs: Vec<usize>,
}

fn quote_str(s: &str) -> String {
    let q = if !s.contains('\'') {
        '\''
    } else if !s.contains('"') {
        '"'
    } else if !s.contains('`') {
        '`'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(q);
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            '\\' => out.push_str("\\\\"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn key_text(k: &PropKey) -> String {
    match k {
        PropKey::Str(s) => {
            let s = s.to_rust();
            if is_ident(&s) { s } else { quote_str(&s) }
        }
        PropKey::Sym(s) => super::symbol::symbol_descriptive(s),
    }
}

fn fmt_number(n: f64) -> String {
    if n == 0.0 && n.is_sign_negative() { "-0".into() } else { number_to_string(n) }
}

impl Inspector {
    /// Devolve o texto e quantos níveis de objetos há dentro.
    fn fmt(&mut self, v: &Value, depth: usize, indent: usize, nested: bool) -> (String, usize) {
        match v {
            Value::Undefined | Value::Empty => ("undefined".into(), 0),
            Value::Null => ("null".into(), 0),
            Value::Bool(b) => (b.to_string(), 0),
            Value::Number(n) => (fmt_number(*n), 0),
            Value::String(s) => {
                if nested {
                    (quote_str(&s.to_rust()), 0)
                } else {
                    (s.to_rust(), 0)
                }
            }
            Value::Symbol(s) => (super::symbol::symbol_descriptive(s), 0),
            Value::BigInt(b) => (format!("{}n", b.to_string_radix(10)), 0),
            Value::Object(o) => self.fmt_object(o, depth, indent),
        }
    }

    fn ctor_name(&self, o: &ObjRef) -> Option<String> {
        let proto = o.proto()?;
        match data_in_chain(&proto, "constructor") {
            Some(Value::Object(c)) => match data_in_chain(&c, "name") {
                Some(Value::String(s)) if !s.is_empty() => Some(s.to_rust()),
                _ => None,
            },
            _ => None,
        }
    }

