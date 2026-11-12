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

