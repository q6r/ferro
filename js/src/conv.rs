//! Conversões numéricas do JavaScript que não dependem da VM:
//! número → texto (Number::toString), texto → número, ToInt32 etc.

use crate::string::trim_js;

/// `Number.prototype.toString()` na base 10 (algoritmo do spec, com os
/// dígitos mais curtos que reproduzem o número — o `{:e}` do Rust já dá isso).
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if n < 0.0 {
        return format!("-{}", number_to_string(-n));
    }
    if n < 1e15 && n.fract() == 0.0 {
        return format!("{}", n as u64);
    }
    let s = format!("{n:e}");
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let k = digits.len() as i32;
    let point = exp + 1;
    if k <= point && point <= 21 {
        let mut out = digits;
        for _ in 0..(point - k) {
            out.push('0');
        }
        out
    } else if 0 < point && point <= 21 {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    } else if -6 < point && point <= 0 {
        let mut out = String::from("0.");
        for _ in 0..(-point) {
            out.push('0');
        }
        out.push_str(&digits);
        out
    } else {
        let e = point - 1;
        let sign = if e >= 0 { '+' } else { '-' };
        if k == 1 {
            format!("{digits}e{sign}{}", e.abs())
        } else {
            format!("{}.{}e{sign}{}", &digits[..1], &digits[1..], e.abs())
        }
    }
}

