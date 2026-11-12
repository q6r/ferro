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

fn bigint_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if a.new_target.is_some() {
        return Err(vm.type_error("BigInt is not a constructor"));
    }
    let p = vm.to_primitive(&a.arg(0), Hint::Number)?;
    if let Value::Number(n) = p {
        return match BigInt::from_f64(n) {
            Some(b) => Ok(Value::BigInt(Rc::new(b))),
            None => {
                let d = crate::conv::number_to_string(n);
                Err(vm.range_error(format!("The number {d} cannot be converted to a BigInt because it is not an integer")))
            }
        };
    }
    Ok(Value::BigInt(to_bigint(vm, &p)?))
}

fn this_bigint(vm: &mut Vm, a: &CallArgs, what: &str) -> Result<Rc<BigInt>, Value> {
    match &a.this {
        Value::BigInt(b) => Ok(b.clone()),
        Value::Object(o) => match &o.borrow().kind {
            Kind::BigIntBox(b) => Ok(b.clone()),
            _ => Err(vm.type_error(format!("BigInt.prototype.{what} requires that 'this' be a BigInt"))),
        },
        _ => Err(vm.type_error(format!("BigInt.prototype.{what} requires that 'this' be a BigInt"))),
    }
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let b = this_bigint(vm, a, "toString")?;
    let radix = if a.arg(0).is_undefined() { 10.0 } else { vm.to_integer(&a.arg(0))? };
    if !(2.0..=36.0).contains(&radix) {
        return Err(vm.range_error("toString() radix must be between 2 and 36"));
    }
    Ok(Value::str(&b.to_string_radix(radix as u32)))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let b = this_bigint(vm, a, "toLocaleString")?;
    let s = b.to_string_radix(10);
    let (sign, digits) = match s.strip_prefix('-') {
        Some(d) => ("-", d.to_string()),
        None => ("", s.clone()),
    };
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    Ok(Value::str(&format!("{sign}{out}")))
}

fn value_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::BigInt(this_bigint(vm, a, "valueOf")?))
}

fn bits_arg(vm: &mut Vm, v: &Value) -> Result<u64, Value> {
    let n = vm.to_integer(v)?;
    if !(0.0..=9007199254740991.0).contains(&n) {
        return Err(vm.range_error("Invalid value: not (convertible to) a safe integer"));
    }
    Ok(n as u64)
}

fn as_int_n(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let bits = bits_arg(vm, &a.arg(0))?;
    let b = to_bigint(vm, &a.arg(1))?;
    Ok(Value::BigInt(Rc::new(b.as_int_n(bits.min(1 << 24)))))
}

fn as_uint_n(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let bits = bits_arg(vm, &a.arg(0))?;
    let b = to_bigint(vm, &a.arg(1))?;
    Ok(Value::BigInt(Rc::new(b.as_uint_n(bits.min(1 << 24)))))
}
