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

