//! BigInt (construtor e protótipo).

use std::rc::Rc;

use super::Realm;
use crate::bigint::BigInt;
use crate::value::*;
use crate::vm::{Hint, Vm};

pub fn install(r: &Realm) {
    let p = &r.bigint_proto;
    let c = r.ctor("BigInt", 1, bigint_ctor, p);
    r.method(&c, "asIntN", 2, as_int_n);
    r.method(&c, "asUintN", 2, as_uint_n);
    r.method(p, "toString", 0, to_string);
    r.method(p, "toLocaleString", 0, to_locale_string);
    r.method(p, "valueOf", 0, value_of);
    r.tag(p, "BigInt");
}

/// ToBigInt.
pub fn to_bigint(vm: &mut Vm, v: &Value) -> Result<Rc<BigInt>, Value> {
    let p = vm.to_primitive(v, Hint::Number)?;
    match &p {
        Value::BigInt(b) => Ok(b.clone()),
        Value::Bool(b) => Ok(Rc::new(BigInt::from_i64(*b as i64))),
        Value::String(s) => match BigInt::parse_js(&s.to_rust()) {
            Some(b) => Ok(Rc::new(b)),
            None => Err(vm.syntax_error(format!("Cannot convert {} to a BigInt", s.to_rust()))),
        },
        Value::Number(n) => {
            let d = crate::conv::number_to_string(*n);
            Err(vm.type_error(format!("Cannot convert {d} to a BigInt")))
        }
        _ => {
            let d = vm.describe(&p);
            Err(vm.type_error(format!("Cannot convert {d} to a BigInt")))
        }
    }
}

