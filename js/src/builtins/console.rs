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

