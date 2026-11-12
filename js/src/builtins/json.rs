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

