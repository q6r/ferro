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

