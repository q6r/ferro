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

/// `Number.prototype.toString(base)` para bases diferentes de 10.
pub fn number_to_string_radix(n: f64, radix: u32) -> String {
    if radix == 10 || !n.is_finite() {
        return number_to_string(n);
    }
    if n == 0.0 {
        return "0".into();
    }
    let neg = n < 0.0;
    let n = n.abs();
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut int = n.trunc();
    let mut frac = n - int;
    let mut out = Vec::new();
    if int == 0.0 {
        out.push(b'0');
    }
    while int >= 1.0 {
        let d = (int % radix as f64) as usize;
        out.push(digits[d]);
        int = (int / radix as f64).trunc();
    }
    out.reverse();
    if frac > 0.0 {
        out.push(b'.');
        // precisão aproximada: até esgotar os bits significativos
        let mut delta = 0.5 * (n.next_up() - n).max(f64::MIN_POSITIVE);
        let mut count = 0;
        loop {
            frac *= radix as f64;
            delta *= radix as f64;
            let d = frac.trunc() as usize;
            frac -= d as f64;
            out.push(digits[d.min(radix as usize - 1)]);
            count += 1;
            if frac <= delta || count >= 52 {
                break;
            }
        }
    }
    let s = String::from_utf8(out).unwrap_or_default();
    if neg { format!("-{s}") } else { s }
}

