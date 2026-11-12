//! JSON.parse e JSON.stringify.

use super::Realm;
use crate::conv::number_to_string;
use crate::ops::index_key;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let json = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.tag(&json, "JSON");
    r.method(&json, "parse", 2, parse);
    r.method(&json, "stringify", 3, stringify);
    r.global_value("JSON", Value::Object(json));
}

// ------------------------------------------------------------------ parse

struct JsonParser<'a> {
    s: &'a [u16],
    pos: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.pos < self.s.len() && matches!(self.s[self.pos], 0x20 | 0x09 | 0x0A | 0x0D) {
            self.pos += 1;
        }
    }

    fn err(&self) -> String {
        if self.pos >= self.s.len() {
            return "Unexpected end of JSON input".into();
        }
        let c = char::from_u32(self.s[self.pos] as u32).unwrap_or('?');
        format!("Unexpected token '{c}', {} is not valid JSON", self.context())
    }

    /// Erro com posição, no formato do V8 ("... at position 7 (line 1 column 8)").
    fn err_at(&self, what: &str) -> String {
        if self.pos >= self.s.len() {
            return "Unexpected end of JSON input".into();
        }
        let before = &self.s[..self.pos];
        let line = before.iter().filter(|&&c| c == 0x0A).count() + 1;
        let col = self.pos - before.iter().rposition(|&c| c == 0x0A).map_or(0, |p| p + 1) + 1;
        format!("{what} in JSON at position {} (line {line} column {col})", self.pos)
    }

    fn err_at_end(&self, what: &str) -> String {
        let pos = self.s.len();
        let line = self.s.iter().filter(|&&c| c == 0x0A).count() + 1;
        let col = pos - self.s.iter().rposition(|&c| c == 0x0A).map_or(0, |p| p + 1) + 1;
        format!("{what} in JSON at position {pos} (line {line} column {col})")
    }

    fn context(&self) -> String {
        if self.s.len() <= 30 {
            return format!("\"{}\"", String::from_utf16_lossy(self.s));
        }
        let start = self.pos.saturating_sub(10);
        let end = (self.pos + 10).min(self.s.len());
        format!(
            "{}\"{}\"{}",
            if start > 0 { "..." } else { "" },
            String::from_utf16_lossy(&self.s[start..end]),
            if end < self.s.len() { "..." } else { "" }
        )
    }

    fn value(&mut self, vm: &mut Vm, depth: u32) -> Result<Value, String> {
        if depth > 5000 {
            return Err("JSON nesting too deep".into());
        }
        self.ws();
        let Some(&c) = self.s.get(self.pos) else { return Err(self.err()) };
        match c {
            0x7B => {
                self.pos += 1;
                let o = vm.new_object();
                self.ws();
                if self.s.get(self.pos) == Some(&0x7D) {
                    self.pos += 1;
                    return Ok(Value::Object(o));
                }
                let mut first = true;
                loop {
                    self.ws();
                    if self.s.get(self.pos) != Some(&0x22) {
                        return Err(self.err_at(if first {
                            "Expected property name or '}'"
                        } else {
                            "Expected double-quoted property name"
                        }));
                    }
                    first = false;
                    let k = self.string()?;
                    self.ws();
                    if self.s.get(self.pos) != Some(&0x3A) {
                        return Err(self.err_at("Expected ':' after property name"));
                    }
                    self.pos += 1;
                    let v = self.value(vm, depth + 1)?;
                    vm.define_data(&o, PropKey::Str(k), v);
                    self.ws();
                    match self.s.get(self.pos) {
                        Some(0x2C) => self.pos += 1,
                        Some(0x7D) => {
                            self.pos += 1;
                            return Ok(Value::Object(o));
                        }
                        _ => return Err(self.err_at("Expected ',' or '}' after property value")),
                    }
                }
            }
            0x5B => {
                self.pos += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.pos) == Some(&0x5D) {
                    self.pos += 1;
                    return Ok(vm.array_value(items));
                }
                loop {
                    items.push(self.value(vm, depth + 1)?);
                    self.ws();
                    match self.s.get(self.pos) {
                        Some(0x2C) => self.pos += 1,
                        Some(0x5D) => {
                            self.pos += 1;
                            return Ok(vm.array_value(items));
                        }
                        _ => return Err(self.err_at("Expected ',' or ']' after array element")),
                    }
                }
            }
            0x22 => Ok(Value::String(self.string()?)),
            0x74 => self.word("true", Value::Bool(true)),
            0x66 => self.word("false", Value::Bool(false)),
            0x6E => self.word("null", Value::Null),
            0x2D | 0x30..=0x39 => self.number(),
            _ => Err(self.err()),
        }
    }

    fn word(&mut self, w: &str, v: Value) -> Result<Value, String> {
        for (i, b) in w.bytes().enumerate() {
            if self.s.get(self.pos + i) != Some(&(b as u16)) {
                self.pos += i;
                return Err(self.err());
            }
        }
        self.pos += w.len();
        Ok(v)
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.s[self.pos] == 0x2D {
            self.pos += 1;
        }
        let digits = |p: &mut Self| {
            let s = p.pos;
            while p.pos < p.s.len() && (0x30..=0x39).contains(&p.s[p.pos]) {
                p.pos += 1;
            }
            p.pos - s
        };
        let int_start = self.pos;
        if digits(self) == 0 {
            return Err(self.err());
        }
        if self.s[int_start] == 0x30 && self.pos - int_start > 1 {
            self.pos = int_start + 1;
            return Err(self.err());
        }
        if self.s.get(self.pos) == Some(&0x2E) {
            self.pos += 1;
            if digits(self) == 0 {
                return Err(self.err());
            }
        }
        if matches!(self.s.get(self.pos), Some(0x65 | 0x45)) {
            self.pos += 1;
            if matches!(self.s.get(self.pos), Some(0x2B | 0x2D)) {
                self.pos += 1;
            }
            if digits(self) == 0 {
                return Err(self.err());
            }
        }
        let text: String = self.s[start..self.pos].iter().map(|&c| c as u8 as char).collect();
        Ok(Value::Number(text.parse().unwrap_or(f64::NAN)))
    }

    fn string(&mut self) -> Result<JsStr, String> {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            let Some(&c) = self.s.get(self.pos) else {
                return Err(self.err_at_end("Unterminated string"));
            };
            self.pos += 1;
            match c {
                0x22 => return Ok(JsStr::from_units(out)),
                0x5C => {
                    let Some(&e) = self.s.get(self.pos) else { return Err(self.err()) };
                    self.pos += 1;
                    match e {
                        0x22 => out.push(0x22),
                        0x5C => out.push(0x5C),
                        0x2F => out.push(0x2F),
                        0x62 => out.push(0x08),
                        0x66 => out.push(0x0C),
                        0x6E => out.push(0x0A),
                        0x72 => out.push(0x0D),
                        0x74 => out.push(0x09),
                        0x75 => {
                            let mut v = 0u16;
                            for _ in 0..4 {
                                let Some(&h) = self.s.get(self.pos) else { return Err(self.err()) };
                                let d = char::from_u32(h as u32).and_then(|ch| ch.to_digit(16));
                                let Some(d) = d else { return Err(self.err()) };
                                v = v * 16 + d as u16;
                                self.pos += 1;
                            }
                            out.push(v);
                        }
                        _ => {
                            self.pos -= 1;
                            return Err(self.err());
                        }
                    }
                }
                0..=0x1F => {
                    self.pos -= 1;
                    return Err(format!("Bad control character in string literal in JSON at position {}", self.pos));
                }
                _ => out.push(c),
            }
        }
    }
}

fn parse(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let text = vm.to_string(&a.arg(0))?;
    let mut p = JsonParser { s: text.units(), pos: 0 };
    let v = match p.value(vm, 0) {
        Ok(v) => v,
        Err(e) => return Err(vm.syntax_error(e)),
    };
    p.ws();
    if p.pos < p.s.len() {
        let e = p.err_at("Unexpected non-whitespace character after JSON");
        return Err(vm.syntax_error(e));
    }
    let reviver = a.arg(1);
    if reviver.is_callable() {
        let root = vm.new_object();
        vm.define_data(&root, PropKey::str(""), v);
        return revive(vm, &root, PropKey::str(""), &reviver);
    }
    Ok(v)
}

fn revive(vm: &mut Vm, holder: &ObjRef, key: PropKey, reviver: &Value) -> Result<Value, Value> {
    let val = vm.get_obj(holder, &key)?;
    if let Value::Object(o) = &val {
        let keys: Vec<PropKey> = if matches!(o.borrow().kind, Kind::Array(_)) {
            let len = vm.length_of(o)?;
            (0..len).map(index_key).collect()
        } else {
            super::object::enumerable_keys(vm, o)?
        };
        for k in keys {
            let nv = revive(vm, o, k.clone(), reviver)?;
            if nv.is_undefined() {
                vm.delete_prop(o, &k);
            } else {
                vm.define_data(o, k, nv);
            }
        }
    }
    vm.call(reviver, Value::Object(holder.clone()), vec![key.to_value(), val])
}

// -------------------------------------------------------------- stringify

struct Ctx {
    replacer: Option<Value>,
    allow: Option<Vec<PropKey>>,
    gap: String,
    stack: Vec<usize>,
}

pub fn quote(s: &JsStr, out: &mut Vec<u16>) {
    out.push(0x22);
    let u = s.units();
    let mut i = 0;
    while i < u.len() {
        let c = u[i];
        match c {
            0x22 => out.extend_from_slice(&[0x5C, 0x22]),
            0x5C => out.extend_from_slice(&[0x5C, 0x5C]),
            0x08 => out.extend_from_slice(&[0x5C, 0x62]),
            0x0C => out.extend_from_slice(&[0x5C, 0x66]),
            0x0A => out.extend_from_slice(&[0x5C, 0x6E]),
            0x0D => out.extend_from_slice(&[0x5C, 0x72]),
            0x09 => out.extend_from_slice(&[0x5C, 0x74]),
            0..=0x1F => out.extend(format!("\\u{c:04x}").encode_utf16()),
            0xD800..=0xDBFF if i + 1 < u.len() && (0xDC00..=0xDFFF).contains(&u[i + 1]) => {
                out.push(c);
                out.push(u[i + 1]);
                i += 1;
            }
            0xD800..=0xDFFF => out.extend(format!("\\u{c:04x}").encode_utf16()),
            _ => out.push(c),
        }
        i += 1;
    }
    out.push(0x22);
}

fn serialize(vm: &mut Vm, ctx: &mut Ctx, holder: &ObjRef, key: &PropKey, indent: &str) -> Result<Option<Vec<u16>>, Value> {
    let mut v = vm.get_obj(holder, key)?;
    if matches!(v, Value::Object(_)) {
        let to_json = vm.get_str(&v, "toJSON")?;
        if to_json.is_callable() {
            v = vm.call(&to_json, v, vec![key.to_value()])?;
        }
    }
    if let Some(r) = &ctx.replacer {
        let r = r.clone();
        v = vm.call(&r, Value::Object(holder.clone()), vec![key.to_value(), v])?;
    }
    // objetos embrulhando primitivos
    if let Value::Object(o) = &v {
        let prim = match &o.borrow().kind {
            Kind::Number(_) => Some(1),
            Kind::String(_) => Some(2),
            Kind::Boolean(b) => {
                let b = *b;
                return Ok(Some(if b { "true" } else { "false" }.encode_utf16().collect()));
            }
            _ => None,
        };
        match prim {
            Some(1) => v = Value::Number(vm.to_number(&v)?),
            Some(2) => v = Value::String(vm.to_string(&v)?),
            _ => {}
        }
    }
    if let Value::BigInt(_) = v {
        return Err(vm.type_error("Do not know how to serialize a BigInt"));
    }
    Ok(Some(match &v {
        Value::Null => "null".encode_utf16().collect(),
        Value::Bool(b) => if *b { "true" } else { "false" }.encode_utf16().collect(),
        Value::String(s) => {
            let mut out = Vec::with_capacity(s.len() + 2);
            quote(s, &mut out);
            out
        }
        Value::Number(n) => {
            if n.is_finite() {
                number_to_string(*n).encode_utf16().collect()
            } else {
                "null".encode_utf16().collect()
            }
        }
        Value::Object(o) if !o.is_callable() => {
            let id = o.id();
            if ctx.stack.contains(&id) {
                return Err(vm.type_error("Converting circular structure to JSON"));
            }
            ctx.stack.push(id);
            let inner = format!("{indent}{}", ctx.gap);
            let is_array = matches!(o.borrow().kind, Kind::Array(_));
            let mut parts: Vec<Vec<u16>> = Vec::new();
            if is_array {
                let len = vm.length_of(o)?;
                for i in 0..len {
                    let s = serialize(vm, ctx, o, &index_key(i), &inner)?;
                    parts.push(s.unwrap_or_else(|| "null".encode_utf16().collect()));
                }
            } else {
                let keys = match &ctx.allow {
                    Some(list) => list.clone(),
                    None => super::object::enumerable_keys(vm, o)?,
                };
                for k in keys {
                    if let Some(s) = serialize(vm, ctx, o, &k, &inner)? {
                        let mut p = Vec::new();
                        if let PropKey::Str(ks) = &k {
                            quote(ks, &mut p);
                        }
                        p.push(0x3A);
                        if !ctx.gap.is_empty() {
                            p.push(0x20);
                        }
                        p.extend(s);
                        parts.push(p);
                    }
                }
            }
            ctx.stack.pop();
            let (open, close) = if is_array { (0x5B, 0x5D) } else { (0x7B, 0x7D) };
            let mut out = vec![open];
            if !parts.is_empty() {
                if ctx.gap.is_empty() {
                    for (i, p) in parts.into_iter().enumerate() {
                        if i > 0 {
                            out.push(0x2C);
                        }
                        out.extend(p);
                    }
                } else {
                    let sep: Vec<u16> = format!(",\n{inner}").encode_utf16().collect();
                    out.extend(format!("\n{inner}").encode_utf16());
                    for (i, p) in parts.into_iter().enumerate() {
                        if i > 0 {
                            out.extend_from_slice(&sep);
                        }
                        out.extend(p);
                    }
                    out.extend(format!("\n{indent}").encode_utf16());
                }
            }
            out.push(close);
            out
        }
        _ => return Ok(None),
    }))
}

fn stringify(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut ctx = Ctx {
        replacer: None,
        allow: None,
        gap: String::new(),
        stack: Vec::new(),
    };
    let r = a.arg(1);
    if r.is_callable() {
        ctx.replacer = Some(r);
    } else if let Value::Object(ro) = &r
        && matches!(ro.borrow().kind, Kind::Array(_))
    {
        let mut list: Vec<PropKey> = Vec::new();
        for item in vm.iterate_to_vec(&r)? {
            let k = match &item {
                Value::String(_) | Value::Number(_) => Some(vm.to_property_key(&item)?),
                Value::Object(o) if matches!(o.borrow().kind, Kind::Number(_) | Kind::String(_)) => {
                    let s = vm.to_string(&item)?;
                    Some(PropKey::Str(s))
                }
                _ => None,
            };
            if let Some(k) = k
                && !list.contains(&k)
            {
                list.push(k);
            }
        }
        ctx.allow = Some(list);
    }
    let mut space = a.arg(2);
    if let Value::Object(o) = &space {
        let k = match &o.borrow().kind {
            Kind::Number(n) => Some(Value::Number(*n)),
            Kind::String(s) => Some(Value::String(s.clone())),
            _ => None,
        };
        if let Some(k) = k {
            space = k;
        }
    }
    ctx.gap = match &space {
        Value::Number(n) => " ".repeat(n.clamp(0.0, 10.0) as usize),
        Value::String(s) => s.to_rust().chars().take(10).collect(),
        _ => String::new(),
    };
    let root = vm.new_object();
    vm.define_data(&root, PropKey::str(""), a.arg(0));
    match serialize(vm, &mut ctx, &root, &PropKey::str(""), "")? {
        Some(s) => Ok(Value::String(JsStr::from_units(s))),
        None => Ok(Value::Undefined),
    }
}
