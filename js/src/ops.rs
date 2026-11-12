//! Operações abstratas do spec usadas pela VM e pelos embutidos:
//! acesso a propriedades, conversões, igualdade, iteração e erros.

use std::collections::HashSet;
use std::rc::Rc;

use crate::ast::FnKind;
use crate::compiler::Code;
use crate::conv::{number_to_string, string_to_number, to_uint32};
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Hint, Vm, same_value_zero, strict_equals, truthy};

/// Operações numéricas binárias (Number ou BigInt).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    UShr,
}

pub fn num_op(k: Arith, x: f64, y: f64) -> f64 {
    use crate::conv::{to_int32, to_uint32};
    match k {
        Arith::Add => x + y,
        Arith::Sub => x - y,
        Arith::Mul => x * y,
        Arith::Div => x / y,
        Arith::Mod => crate::vm::js_rem(x, y),
        Arith::Exp => crate::vm::js_pow(x, y),
        Arith::And => (to_int32(x) & to_int32(y)) as f64,
        Arith::Or => (to_int32(x) | to_int32(y)) as f64,
        Arith::Xor => (to_int32(x) ^ to_int32(y)) as f64,
        Arith::Shl => to_int32(x).wrapping_shl(to_uint32(y) & 31) as f64,
        Arith::Shr => (to_int32(x) >> (to_uint32(y) & 31)) as f64,
        Arith::UShr => ((to_int32(x) as u32) >> (to_uint32(y) & 31)) as f64,
    }
}

/// Representação curta de um valor para mensagens de erro.
pub fn key_display(k: &PropKey) -> String {
    match k {
        PropKey::Str(s) => s.to_rust(),
        PropKey::Sym(s) => format!("Symbol({})", s.description().map(|d| d.to_rust()).unwrap_or_default()),
    }
}

pub fn index_key(i: usize) -> PropKey {
    PropKey::Str(JsStr::new(&i.to_string()))
}

impl Vm {
    // ------------------------------------------------------------ erros

    pub fn make_error(&mut self, proto: &ObjRef, msg: &str) -> Value {
        let o = ObjRef::new(Object::new(Some(proto.clone()), Kind::Error));
        if !msg.is_empty() {
            o.borrow_mut().props.insert(PropKey::str("message"), Property::hidden(Value::str(msg)));
        }
        let name = data_in_chain(proto, "name").map(|v| match v {
            Value::String(s) => s.to_rust(),
            _ => "Error".into(),
        });
        let name = name.unwrap_or_else(|| "Error".into());
        let header = if msg.is_empty() { name } else { format!("{name}: {msg}") };
        let stack = self.stack_trace(&header);
        o.borrow_mut().props.insert(PropKey::str("stack"), Property::hidden(Value::str(&stack)));
        Value::Object(o)
    }

    /// Texto de `error.stack`: cabeçalho e as chamadas ativas.
    pub fn stack_trace(&self, header: &str) -> String {
        let mut s = header.to_string();
        let n = self.trace.len();
        for (count, (i, t)) in self.trace.iter().enumerate().rev().enumerate() {
            if count >= 10 {
                break;
            }
            let pc = if i == n - 1 { self.cur_pc } else { t.pc };
            let line = t.code.line_at(pc);
            let fname = t.code.name.to_rust();
            if t.code.is_script || fname.is_empty() {
                s.push_str(&format!("\n    at {}:{}", t.code.file, line));
            } else {
                s.push_str(&format!("\n    at {} ({}:{})", fname, t.code.file, line));
            }
        }
        s
    }

    pub fn type_error(&mut self, msg: impl Into<String>) -> Value {
        let p = self.realm.type_error_proto.clone();
        self.make_error(&p, &msg.into())
    }

    pub fn range_error(&mut self, msg: impl Into<String>) -> Value {
        let p = self.realm.range_error_proto.clone();
        self.make_error(&p, &msg.into())
    }

    pub fn reference_error(&mut self, msg: impl Into<String>) -> Value {
        let p = self.realm.reference_error_proto.clone();
        self.make_error(&p, &msg.into())
    }

    pub fn syntax_error(&mut self, msg: impl Into<String>) -> Value {
        let p = self.realm.syntax_error_proto.clone();
        self.make_error(&p, &msg.into())
    }

