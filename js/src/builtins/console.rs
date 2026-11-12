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

    fn fmt_object(&mut self, o: &ObjRef, depth: usize, indent: usize) -> (String, usize) {
        // Proxy aparece como o alvo (como no Node)
        let target = match &o.borrow().kind {
            Kind::Proxy { target: Some(t), .. } => Some(t.clone()),
            _ => None,
        };
        if let Some(t) = target {
            return self.fmt_object(&t, depth, indent);
        }
        let id = o.id();
        if self.seen.contains(&id) {
            let n = match self.refs.iter().position(|r| *r == id) {
                Some(i) => i + 1,
                None => {
                    self.refs.push(id);
                    self.refs.len()
                }
            };
            return (format!("[Circular *{n}]"), 0);
        }
        let b = o.borrow();
        let ctor = self.ctor_name(o);
        let null_proto = b.proto.is_none();
        // base (prefixo) e chaves
        enum Shape {
            Plain,
            Array(Vec<Value>),
            Map(Vec<(Value, Value)>),
            Set(Vec<Value>),
        }
        let mut base = String::new();
        let mut iter_prefix: Option<&str> = None;
        let mut typed_prefix: Option<String> = None;
        let mut promise_entry: Option<Result<(Value, bool), &str>> = None;
        let shape = match &b.kind {
            Kind::Array(v) => Shape::Array(v.clone()),
            Kind::TypedArray(t) => {
                typed_prefix = Some(format!("{}({})", t.kind.name(), t.length));
                Shape::Array((0..t.length).filter_map(|i| t.get(i)).collect())
            }
            Kind::ArrayBuffer(bytes) => {
                let d = bytes.borrow();
                let hex: Vec<String> = d.iter().take(50).map(|x| format!("{x:02x}")).collect();
                let more = if d.len() > 50 { format!(" ... {} more bytes", d.len() - 50) } else { String::new() };
                base = format!("ArrayBuffer {{ [Uint8Contents]: <{}{more}>, byteLength: {} }}", hex.join(" "), d.len());
                Shape::Plain
            }
            Kind::DataView(t) => {
                base = format!("DataView {{ byteLength: {}, byteOffset: {} }}", t.length, t.offset);
                Shape::Plain
            }
            Kind::Map(c) => Shape::Map(c.entries.iter().flatten().cloned().collect()),
            Kind::Set(c) => Shape::Set(c.entries.iter().flatten().map(|e| e.0.clone()).collect()),
            Kind::Function(f) => {
                let name = match data_in_chain(o, "name") {
                    Some(Value::String(s)) => s.to_rust(),
                    _ => String::new(),
                };
                let is_class = matches!(f, Func::Closure(c) if c.code.is_class_constructor());
                base = if is_class {
                    let ext = o.proto().and_then(|p| match data_in_chain(&p, "name") {
                        Some(Value::String(s)) if !s.is_empty() => Some(s.to_rust()),
                        _ => None,
                    });
                    match ext {
                        Some(e) => format!("[class {} extends {e}]", if name.is_empty() { "(anonymous)" } else { &name }),
                        None => format!("[class {}]", if name.is_empty() { "(anonymous)" } else { &name }),
                    }
                } else {
                    let kind = match f {
                        Func::Closure(c) if c.code.is_async && c.code.is_generator => "AsyncGeneratorFunction",
                        Func::Closure(c) if c.code.is_async => "AsyncFunction",
                        Func::Closure(c) if c.code.is_generator => "GeneratorFunction",
                        _ => "Function",
                    };
                    if name.is_empty() { format!("[{kind} (anonymous)]") } else { format!("[{kind}: {name}]") }
                };
                Shape::Plain
            }
            Kind::Error => {
                let stack = match b.props.get(&PropKey::str("stack")) {
                    Some(Property {
                        slot: Slot::Data { value: Value::String(s), .. },
                        ..
                    }) => Some(s.to_rust()),
                    _ => None,
                };
                base = stack.unwrap_or_else(|| {
                    let name = match data_in_chain(o, "name") {
                        Some(Value::String(s)) => s.to_rust(),
                        _ => "Error".into(),
                    };
                    let msg = match data_in_chain(o, "message") {
                        Some(Value::String(s)) => s.to_rust(),
                        _ => String::new(),
                    };
                    if msg.is_empty() { name } else { format!("{name}: {msg}") }
                });
                if depth > 0 {
                    base = format!("[{}]", base.lines().next().unwrap_or(""));
                }
                Shape::Plain
            }
            Kind::Boolean(x) => {
                base = format!("[Boolean: {x}]");
                Shape::Plain
            }
            Kind::Number(n) => {
                base = format!("[Number: {}]", fmt_number(*n));
                Shape::Plain
            }
            Kind::String(s) => {
                base = format!("[String: {}]", quote_str(&s.to_rust()));
                Shape::Plain
            }
            Kind::Symbol(s) => {
                base = format!("[Symbol: {}]", super::symbol::symbol_descriptive(s));
                Shape::Plain
            }
            Kind::BigIntBox(v) => {
                base = format!("[BigInt: {}n]", v.to_string_radix(10));
                Shape::Plain
            }
            Kind::Date(t) => {
                base = if t.is_nan() { "Invalid Date".into() } else { super::date::iso_string(*t) };
                Shape::Plain
            }
            Kind::RegExp { source, flags, .. } => {
                base = format!("/{}/{}", source.to_rust(), flags.to_rust());
                Shape::Plain
            }
            Kind::WeakRef(_) => {
                base = "WeakRef".into();
                Shape::Plain
            }
            Kind::ArrayIter { .. } => {
                iter_prefix = Some("Object [Array Iterator]");
                Shape::Plain
            }
            Kind::MapIter { .. } => {
                iter_prefix = Some("Object [Map Iterator]");
                Shape::Plain
            }
            Kind::StringIter { .. } => {
                iter_prefix = Some("Object [String Iterator]");
                Shape::Plain
            }
            Kind::Arguments => {
                iter_prefix = Some("[Arguments]");
                Shape::Plain
            }
            Kind::Generator(_) => {
                iter_prefix = Some("Object [Generator]");
                Shape::Plain
            }
            Kind::AsyncGenerator(_) => {
                iter_prefix = Some("Object [AsyncGenerator]");
                Shape::Plain
            }
            Kind::Promise(d) => {
                promise_entry = Some(match &d.state {
                    super::promise::PState::Pending => Err("<pending>"),
                    super::promise::PState::Fulfilled(v) => Ok((v.clone(), false)),
                    super::promise::PState::Rejected(v) => Ok((v.clone(), true)),
                });
                Shape::Plain
            }
            _ => Shape::Plain,
        };
        let is_tagged_kind = !base.is_empty();
        // propriedades próprias enumeráveis (sem índices de array)
        let is_array = matches!(shape, Shape::Array(_));
        let keys: Vec<PropKey> = b
            .props
            .ordered_keys()
            .into_iter()
            .filter(|k| !matches!(k, PropKey::Sym(s) if s.is_private()))
            .filter(|k| b.props.get(k).is_some_and(|p| p.enumerable))
            .collect();
        let props: Vec<(PropKey, Property)> = keys.iter().filter_map(|k| b.props.get(k).map(|p| (k.clone(), p.clone()))).collect();
        drop(b);

        let empty_shape = match &shape {
            Shape::Plain => promise_entry.is_none(),
            Shape::Array(v) => v.is_empty(),
            Shape::Map(v) => v.is_empty(),
            Shape::Set(v) => v.is_empty(),
        };
        // prefixo de classe / protótipo nulo
        let mut prefix = String::new();
        match &shape {
            Shape::Array(v) => {
                if let Some(t) = &typed_prefix {
                    prefix = t.clone();
                } else if let Some(c) = &ctor
                    && c != "Array"
                {
                    prefix = format!("{c}({})", v.len());
                }
            }
            Shape::Map(v) => prefix = format!("{}({})", ctor.clone().unwrap_or_else(|| "Map".into()), v.len()),
            Shape::Set(v) => prefix = format!("{}({})", ctor.clone().unwrap_or_else(|| "Set".into()), v.len()),
            Shape::Plain => {
                if promise_entry.is_some() {
                    prefix = "Promise".to_string();
                } else if let Some(p) = iter_prefix {
                    prefix = p.to_string();
                } else if !is_tagged_kind {
                    if null_proto {
                        prefix = "[Object: null prototype]".into();
                    } else if let Some(c) = &ctor
                        && c != "Object"
                    {
                        prefix = c.clone();
                    }
                }
            }
        }
        let (open, close) = if is_array { ("[", "]") } else { ("{", "}") };
        if props.is_empty() && empty_shape {
            if is_tagged_kind {
                return (base, 0);
            }
            let s = if prefix.is_empty() { format!("{open}{close}") } else { format!("{prefix} {open}{close}") };
            return (s, 0);
        }
        if depth > self.max_depth {
            let name = match &shape {
                Shape::Array(_) => "[Array]".to_string(),
                _ if is_tagged_kind => base,
                _ => format!("[{}]", ctor.unwrap_or_else(|| "Object".into())),
            };
            return (name, 0);
        }
        self.seen.push(id);
        let mut out: Vec<String> = Vec::new();
        let mut levels = 0;
        let child_indent = indent + 2;
        match &shape {
            Shape::Array(v) => {
                let mut holes = 0;
                let mut shown = 0;
                for x in v.iter() {
                    if shown >= 100 {
                        break;
                    }
                    if matches!(x, Value::Empty) {
                        holes += 1;
                        continue;
                    }
                    if holes > 0 {
                        out.push(format!("<{holes} empty item{}>", if holes > 1 { "s" } else { "" }));
                        shown += 1;
                        holes = 0;
                    }
                    let (s, l) = self.fmt(x, depth + 1, child_indent, true);
                    levels = levels.max(l);
                    out.push(s);
                    shown += 1;
                }
                if holes > 0 {
                    out.push(format!("<{holes} empty item{}>", if holes > 1 { "s" } else { "" }));
                }
                if v.len() > 100 && shown >= 100 {
                    let rest = v.len() - 100;
                    out.push(format!("... {rest} more item{}", if rest > 1 { "s" } else { "" }));
                }
            }
            Shape::Map(entries) => {
                for (k, val) in entries.iter() {
                    let (ks, l1) = self.fmt(k, depth + 1, child_indent, true);
                    let (vs, l2) = self.fmt(val, depth + 1, child_indent, true);
                    levels = levels.max(l1).max(l2);
                    out.push(format!("{ks} => {vs}"));
                }
            }
            Shape::Set(items) => {
                for x in items.iter() {
                    let (s, l) = self.fmt(x, depth + 1, child_indent, true);
                    levels = levels.max(l);
                    out.push(s);
                }
            }
            Shape::Plain => {
                match &promise_entry {
                    Some(Err(p)) => out.push(p.to_string()),
                    Some(Ok((v, rejected))) => {
                        let (s, l) = self.fmt(v, depth + 1, child_indent, true);
                        levels = levels.max(l);
                        out.push(if *rejected { format!("<rejected> {s}") } else { s });
                    }
                    None => {}
                }
            }
        }
        let array_entries = out.len();
        for (k, p) in props.iter() {
            if is_array && k.as_index().is_some() {
                continue;
            }
            let vs = match &p.slot {
                Slot::Data { value, .. } => {
                    let (s, l) = self.fmt(value, depth + 1, child_indent, true);
                    levels = levels.max(l);
                    s
                }
                Slot::Accessor { get: Some(_), set: Some(_) } => "[Getter/Setter]".into(),
                Slot::Accessor { get: Some(_), .. } => "[Getter]".into(),
                Slot::Accessor { .. } => "[Setter]".into(),
            };
            out.push(format!("{}: {vs}", key_text(k)));
        }
        self.seen.pop();
        if let Some(i) = self.refs.iter().position(|r| *r == id) {
            prefix = if prefix.is_empty() { format!("<ref *{}>", i + 1) } else { format!("<ref *{}> {prefix}", i + 1) };
        }
        let head = match (prefix.is_empty(), base.is_empty()) {
            (true, true) => String::new(),
            (false, true) => prefix,
            (true, false) => base,
            (false, false) => format!("{prefix} {base}"),
        };
        let before = out.len();
        let out = if is_array && array_entries > 6 && out.len() == array_entries {
            group_array(&out, indent)
        } else {
            out
        };
        let grouped = out.len() != before;
        (reduce(&out, &head, open, close, indent, levels, grouped), levels + 1)
    }
}

