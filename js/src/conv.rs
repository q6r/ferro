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

/// Texto → número (o `Number("...")` do JavaScript).
pub fn string_to_number(units: &[u16]) -> f64 {
    let t = trim_js(units);
    if t.is_empty() {
        return 0.0;
    }
    if t.iter().any(|&c| c > 0x7f) {
        return f64::NAN;
    }
    let s: String = t.iter().map(|&c| c as u8 as char).collect();
    match s.as_str() {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let b = s.as_bytes();
    if b.len() > 2 && b[0] == b'0' {
        let radix = match b[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            return parse_radix_digits(&s[2..], radix).unwrap_or(f64::NAN);
        }
    }
    if !is_decimal_literal(b) {
        return f64::NAN;
    }
    s.parse::<f64>().unwrap_or(f64::NAN)
}

fn parse_radix_digits(s: &str, radix: u32) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    let mut v = 0f64;
    for c in s.chars() {
        let d = c.to_digit(radix)?;
        v = v * radix as f64 + d as f64;
    }
    Some(v)
}

/// Sinal opcional, dígitos com ponto opcional e expoente opcional.
fn is_decimal_literal(b: &[u8]) -> bool {
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == b.len()
}

/// `parseFloat`: o maior prefixo que forma um número decimal.
pub fn parse_float(units: &[u16]) -> f64 {
    let t = trim_js(units);
    let s: String = t.iter().take_while(|&&c| c < 0x80).map(|&c| c as u8 as char).collect();
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        return if b.first() == Some(&b'-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let mut end = 0;
    let mut digits = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits = true;
        end = i;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        if digits {
            end = i;
        }
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits = true;
            end = i;
        }
    }
    if !digits {
        return f64::NAN;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            end = j;
        }
    }
    s[..end].trim_end_matches('.').parse::<f64>().unwrap_or(f64::NAN)
}

