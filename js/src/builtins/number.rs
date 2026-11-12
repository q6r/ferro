//! Number, Boolean e Math.

use super::Realm;
use crate::conv;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Vm, truthy};

pub fn install(r: &Realm) {
    let proto = &r.number_proto;
    let c = r.ctor("Number", 1, number_ctor, proto);
    r.constant(&c, "MAX_SAFE_INTEGER", Value::Number(9007199254740991.0));
    r.constant(&c, "MIN_SAFE_INTEGER", Value::Number(-9007199254740991.0));
    r.constant(&c, "EPSILON", Value::Number(f64::EPSILON));
    r.constant(&c, "MAX_VALUE", Value::Number(f64::MAX));
    r.constant(&c, "MIN_VALUE", Value::Number(5e-324));
    r.constant(&c, "NaN", Value::Number(f64::NAN));
    r.constant(&c, "POSITIVE_INFINITY", Value::Number(f64::INFINITY));
    r.constant(&c, "NEGATIVE_INFINITY", Value::Number(f64::NEG_INFINITY));
    r.method(&c, "isFinite", 1, is_finite);
    r.method(&c, "isInteger", 1, is_integer);
    r.method(&c, "isNaN", 1, is_nan);
    r.method(&c, "isSafeInteger", 1, is_safe_integer);
    let pf = r.method(&c, "parseFloat", 1, parse_float);
    let pi = r.method(&c, "parseInt", 2, parse_int);
    r.global_value("parseFloat", Value::Object(pf));
    r.global_value("parseInt", Value::Object(pi));

    r.method(proto, "toString", 1, to_string);
    r.method(proto, "toFixed", 1, to_fixed);
    r.method(proto, "toExponential", 1, to_exponential);
    r.method(proto, "toPrecision", 1, to_precision);
    r.method(proto, "toLocaleString", 0, to_locale_string);
    r.method(proto, "valueOf", 0, value_of);

    let bp = &r.boolean_proto;
    r.ctor("Boolean", 1, boolean_ctor, bp);
    r.method(bp, "toString", 0, bool_to_string);
    r.method(bp, "valueOf", 0, bool_value_of);

    let math = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.tag(&math, "Math");
    for (name, v) in [
        ("E", std::f64::consts::E),
        ("LN10", std::f64::consts::LN_10),
        ("LN2", std::f64::consts::LN_2),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("PI", std::f64::consts::PI),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("SQRT2", std::f64::consts::SQRT_2),
    ] {
        r.constant(&math, name, Value::Number(v));
    }
    let fns: &[(&str, u32, NativeFn)] = &[
        ("abs", 1, m_abs),
        ("acos", 1, m_acos),
        ("acosh", 1, m_acosh),
        ("asin", 1, m_asin),
        ("asinh", 1, m_asinh),
        ("atan", 1, m_atan),
        ("atanh", 1, m_atanh),
        ("atan2", 2, m_atan2),
        ("cbrt", 1, m_cbrt),
        ("ceil", 1, m_ceil),
        ("clz32", 1, m_clz32),
        ("cos", 1, m_cos),
        ("cosh", 1, m_cosh),
        ("exp", 1, m_exp),
        ("expm1", 1, m_expm1),
        ("floor", 1, m_floor),
        ("fround", 1, m_fround),
        ("f16round", 1, m_f16round),
        ("hypot", 2, m_hypot),
        ("imul", 2, m_imul),
        ("log", 1, m_log),
        ("log1p", 1, m_log1p),
        ("log10", 1, m_log10),
        ("log2", 1, m_log2),
        ("max", 2, m_max),
        ("min", 2, m_min),
        ("pow", 2, m_pow),
        ("random", 0, m_random),
        ("round", 1, m_round),
        ("sign", 1, m_sign),
        ("sin", 1, m_sin),
        ("sinh", 1, m_sinh),
        ("sqrt", 1, m_sqrt),
        ("tan", 1, m_tan),
        ("tanh", 1, m_tanh),
        ("trunc", 1, m_trunc),
    ];
    for (name, len, f) in fns {
        r.method(&math, name, *len, *f);
    }
    r.global_value("Math", Value::Object(math));
}

fn number_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let n = if a.args.is_empty() {
        0.0
    } else {
        match vm.to_numeric(&a.args[0])? {
            Value::BigInt(b) => b.to_f64(),
            v => vm.to_number(&v)?,
        }
    };
    match &a.new_target {
        None => Ok(Value::Number(n)),
        Some(nt) => {
            let def = vm.realm.number_proto.clone();
            let proto = vm.proto_from_ctor(Some(nt), &def)?;
            Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::Number(n)))))
        }
    }
}

fn this_number(vm: &mut Vm, a: &CallArgs, what: &str) -> Result<f64, Value> {
    match &a.this {
        Value::Number(n) => Ok(*n),
        Value::Object(o) => match &o.borrow().kind {
            Kind::Number(n) => Ok(*n),
            _ => Err(vm.type_error(format!("Number.prototype.{what} requires that 'this' be a Number"))),
        },
        _ => Err(vm.type_error(format!("Number.prototype.{what} requires that 'this' be a Number"))),
    }
}

fn num_arg(a: &CallArgs) -> Option<f64> {
    match a.arg(0) {
        Value::Number(n) => Some(n),
        _ => None,
    }
}

fn is_finite(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(num_arg(a).is_some_and(|n| n.is_finite())))
}

fn is_integer(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(num_arg(a).is_some_and(|n| n.is_finite() && n.trunc() == n)))
}

fn is_nan(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(num_arg(a).is_some_and(|n| n.is_nan())))
}

fn is_safe_integer(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(num_arg(a).is_some_and(|n| n.is_finite() && n.trunc() == n && n.abs() <= 9007199254740991.0)))
}

fn parse_float(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = vm.to_string(&a.arg(0))?;
    Ok(Value::Number(conv::parse_float(s.units())))
}

fn parse_int(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let s = vm.to_string(&a.arg(0))?;
    let r = conv::to_int32(vm.to_number(&a.arg(1))?);
    Ok(Value::Number(conv::parse_int(s.units(), r)))
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let n = this_number(vm, a, "toString")?;
    let radix = if a.arg(0).is_undefined() { 10.0 } else { vm.to_integer(&a.arg(0))? };
    if !(2.0..=36.0).contains(&radix) {
        return Err(vm.range_error("toString() radix must be between 2 and 36"));
    }
    Ok(Value::str(&conv::number_to_string_radix(n, radix as u32)))
}

fn to_fixed(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let n = this_number(vm, a, "toFixed")?;
    let d = vm.to_integer(&a.arg(0))?;
    if !(0.0..=100.0).contains(&d) {
        return Err(vm.range_error("toFixed() digits argument must be between 0 and 100"));
    }
    Ok(Value::str(&conv::to_fixed(n, d as usize)))
}

fn to_exponential(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let n = this_number(vm, a, "toExponential")?;
    let d = if a.arg(0).is_undefined() {
        None
    } else {
        let d = vm.to_integer(&a.arg(0))?;
        if !(0.0..=100.0).contains(&d) && n.is_finite() {
            return Err(vm.range_error("toExponential() argument must be between 0 and 100"));
        }
        Some(d as usize)
    };
    Ok(Value::str(&conv::to_exponential(n, d)))
}

fn to_precision(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let n = this_number(vm, a, "toPrecision")?;
    if a.arg(0).is_undefined() {
        return Ok(Value::str(&conv::number_to_string(n)));
    }
    let p = vm.to_integer(&a.arg(0))?;
    if !n.is_finite() {
        return Ok(Value::str(&conv::number_to_string(n)));
    }
    if !(1.0..=100.0).contains(&p) {
        return Err(vm.range_error("toPrecision() argument must be between 1 and 100"));
    }
    Ok(Value::str(&conv::to_precision(n, p as usize)))
}

/// Formato en-US: separador de milhar e até 3 casas decimais.
pub fn locale_format(n: f64, max_frac: usize) -> String {
    if !n.is_finite() {
        return if n.is_nan() { "NaN".into() } else if n > 0.0 { "∞".into() } else { "-∞".into() };
    }
    let s = conv::to_fixed(n.abs(), max_frac);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), f.trim_end_matches('0').to_string()),
        None => (s.clone(), String::new()),
    };
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let sign = if n < 0.0 && (int != "0" || !frac.is_empty()) { "-" } else { "" };
    if frac.is_empty() { format!("{sign}{grouped}") } else { format!("{sign}{grouped}.{frac}") }
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let n = this_number(vm, a, "toLocaleString")?;
    Ok(Value::str(&super::intl::number_to_locale_string(vm, n, &a.arg(0), &a.arg(1))?))
}

fn value_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Number(this_number(vm, a, "valueOf")?))
}

fn boolean_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let b = truthy(&a.arg(0));
    match &a.new_target {
        None => Ok(Value::Bool(b)),
        Some(nt) => {
            let def = vm.realm.boolean_proto.clone();
            let proto = vm.proto_from_ctor(Some(nt), &def)?;
            Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::Boolean(b)))))
        }
    }
}

fn this_bool(vm: &mut Vm, a: &CallArgs) -> Result<bool, Value> {
    match &a.this {
        Value::Bool(b) => Ok(*b),
        Value::Object(o) => match &o.borrow().kind {
            Kind::Boolean(b) => Ok(*b),
            _ => Err(vm.type_error("Boolean.prototype.valueOf requires that 'this' be a Boolean")),
        },
        _ => Err(vm.type_error("Boolean.prototype.valueOf requires that 'this' be a Boolean")),
    }
}

fn bool_to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::str(if this_bool(vm, a)? { "true" } else { "false" }))
}

fn bool_value_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(this_bool(vm, a)?))
}

// ----------------------------------------------------------------- Math

fn n0(vm: &mut Vm, a: &CallArgs) -> Result<f64, Value> {
    vm.to_number(&a.arg(0))
}

macro_rules! math1 {
    ($name:ident, $f:expr) => {
        fn $name(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
            let x = n0(vm, a)?;
            #[allow(clippy::redundant_closure_call)]
            Ok(Value::Number($f(x)))
        }
    };
}

math1!(m_abs, f64::abs);
math1!(m_acos, f64::acos);
math1!(m_acosh, f64::acosh);
math1!(m_asin, f64::asin);
math1!(m_asinh, f64::asinh);
math1!(m_atan, f64::atan);
math1!(m_atanh, f64::atanh);
math1!(m_cbrt, f64::cbrt);
math1!(m_ceil, f64::ceil);
math1!(m_cos, f64::cos);
math1!(m_cosh, f64::cosh);
math1!(m_exp, f64::exp);
math1!(m_expm1, f64::exp_m1);
math1!(m_floor, f64::floor);
math1!(m_fround, |x: f64| x as f32 as f64);
math1!(m_f16round, |x: f64| crate::conv::f16_to_f64(crate::conv::f64_to_f16(x)));
math1!(m_log, f64::ln);
math1!(m_log1p, f64::ln_1p);
math1!(m_log10, f64::log10);
math1!(m_log2, f64::log2);
math1!(m_sin, f64::sin);
math1!(m_sinh, f64::sinh);
math1!(m_sqrt, f64::sqrt);
math1!(m_tan, f64::tan);
math1!(m_tanh, f64::tanh);
math1!(m_trunc, f64::trunc);
math1!(m_sign, |x: f64| if x.is_nan() || x == 0.0 { x } else { x.signum() });
math1!(m_clz32, |x: f64| conv::to_uint32(x).leading_zeros() as f64);
math1!(m_round, |x: f64| {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let r = (x + 0.5).floor();
    if r == 0.0 && x < 0.0 { -0.0 } else if x.fract().abs() == 0.5 && x < 0.0 { x.ceil() } else { r }
});

fn m_atan2(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let y = vm.to_number(&a.arg(0))?;
    let x = vm.to_number(&a.arg(1))?;
    Ok(Value::Number(y.atan2(x)))
}

fn m_pow(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let x = vm.to_number(&a.arg(0))?;
    let y = vm.to_number(&a.arg(1))?;
    Ok(Value::Number(crate::vm::js_pow(x, y)))
}

fn m_imul(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let x = conv::to_int32(vm.to_number(&a.arg(0))?);
    let y = conv::to_int32(vm.to_number(&a.arg(1))?);
    Ok(Value::Number(x.wrapping_mul(y) as f64))
}

fn m_hypot(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut sum = 0.0f64;
    let mut inf = false;
    let mut nan = false;
    for v in a.args.clone() {
        let n = vm.to_number(&v)?;
        if n.is_infinite() {
            inf = true;
        }
        if n.is_nan() {
            nan = true;
        }
        sum += n * n;
    }
    Ok(Value::Number(if inf {
        f64::INFINITY
    } else if nan {
        f64::NAN
    } else {
        sum.sqrt()
    }))
}

fn m_max(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut r = f64::NEG_INFINITY;
    for v in a.args.clone() {
        let n = vm.to_number(&v)?;
        if n.is_nan() || r.is_nan() {
            r = f64::NAN;
        } else if n > r || (n == 0.0 && r == 0.0 && !n.is_sign_negative()) {
            r = n;
        }
    }
    Ok(Value::Number(r))
}

fn m_min(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut r = f64::INFINITY;
    for v in a.args.clone() {
        let n = vm.to_number(&v)?;
        if n.is_nan() || r.is_nan() {
            r = f64::NAN;
        } else if n < r || (n == 0.0 && r == 0.0 && n.is_sign_negative()) {
            r = n;
        }
    }
    Ok(Value::Number(r))
}

fn m_random(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    // xorshift64*
    let mut x = vm.rng;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    vm.rng = x;
    let r = x.wrapping_mul(0x2545F4914F6CDD1D);
    Ok(Value::Number((r >> 11) as f64 / (1u64 << 53) as f64))
}

#[allow(dead_code)]
fn unused(_: JsStr) {}
