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

/// `parseInt(texto, base)`.
pub fn parse_int(units: &[u16], radix: i32) -> f64 {
    let t = trim_js(units);
    let mut i = 0;
    let mut neg = false;
    if i < t.len() && (t[i] == b'+' as u16 || t[i] == b'-' as u16) {
        neg = t[i] == b'-' as u16;
        i += 1;
    }
    let mut radix = radix;
    let mut strip_prefix = true;
    if radix != 0 {
        if !(2..=36).contains(&radix) {
            return f64::NAN;
        }
        if radix != 16 {
            strip_prefix = false;
        }
    } else {
        radix = 10;
    }
    if strip_prefix && i + 1 < t.len() && t[i] == b'0' as u16 && (t[i + 1] == b'x' as u16 || t[i + 1] == b'X' as u16) {
        i += 2;
        radix = 16;
    }
    let mut v = 0f64;
    let mut any = false;
    while i < t.len() {
        let c = t[i];
        let d = match c {
            0x30..=0x39 => (c - 0x30) as i32,
            0x61..=0x7a => (c - 0x61) as i32 + 10,
            0x41..=0x5a => (c - 0x41) as i32 + 10,
            _ => break,
        };
        if d >= radix {
            break;
        }
        v = v * radix as f64 + d as f64;
        any = true;
        i += 1;
    }
    if !any {
        return f64::NAN;
    }
    if radix == 10 && v >= 1e21 {
        // precisão: deixa o Rust converter o texto decimal
        let s: String = t[..i].iter().map(|&c| c as u8 as char).collect();
        if let Ok(x) = s.trim_start_matches(['+', '-']).parse::<f64>() {
            v = x;
        }
    }
    if neg { -v } else { v }
}

pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    if n.abs() < 2147483648.0 {
        return n as i32;
    }
    let m = n.trunc().rem_euclid(4294967296.0);
    if m >= 2147483648.0 { (m - 4294967296.0) as i32 } else { m as i32 }
}

pub fn to_uint32(n: f64) -> u32 {
    to_int32(n) as u32
}

pub fn to_uint16(n: f64) -> u16 {
    to_int32(n) as u16
}

/// ToIntegerOrInfinity.
pub fn to_integer(n: f64) -> f64 {
    if n.is_nan() {
        return 0.0;
    }
    if n.is_infinite() {
        return n;
    }
    n.trunc() + 0.0
}

/// Índice relativo (para slice, at, etc.): negativo conta do fim.
pub fn relative_index(n: f64, len: usize) -> usize {
    let len_f = len as f64;
    let n = to_integer(n);
    if n < 0.0 {
        (len_f + n).max(0.0) as usize
    } else {
        n.min(len_f) as usize
    }
}

/// `toFixed`: arredondamento "meio para cima" como o spec manda.
pub fn to_fixed(n: f64, digits: usize) -> String {
    if !n.is_finite() || n.abs() >= 1e21 {
        return number_to_string(n);
    }
    let neg = n < 0.0;
    let x = n.abs();
    let mut s = format!("{x:.digits$}");
    // empate exato (ex.: 2.5, 1.25): o Rust arredonda para par, o JS para cima
    let exact = format!("{x:.1100}");
    if let Some(dot) = exact.find('.') {
        let frac = &exact[dot + 1..];
        if frac.len() > digits && frac.as_bytes()[digits] == b'5' && frac[digits + 1..].bytes().all(|c| c == b'0') {
            let int_part = &exact[..dot];
            let mut t: Vec<u8> = format!("{int_part}{}", &frac[..digits]).into_bytes();
            let mut i = t.len();
            loop {
                if i == 0 {
                    t.insert(0, b'1');
                    break;
                }
                i -= 1;
                if t[i] == b'9' {
                    t[i] = b'0';
                } else {
                    t[i] += 1;
                    break;
                }
            }
            let int_len = t.len() - digits;
            let t = String::from_utf8(t).unwrap_or_default();
            s = if digits == 0 { t } else { format!("{}.{}", &t[..int_len], &t[int_len..]) };
        }
    }
    if neg && s.bytes().any(|c| c != b'0' && c != b'.') {
        format!("-{s}")
    } else if neg {
        format!("-{s}")
    } else {
        s
    }
}

/// `toExponential(digits)`; `None` = quantos dígitos forem necessários.
pub fn to_exponential(n: f64, digits: Option<usize>) -> String {
    if !n.is_finite() {
        return number_to_string(n);
    }
    let s = match digits {
        Some(d) => format!("{:.*e}", d, n),
        None => format!("{n:e}"),
    };
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let e: i32 = exp.parse().unwrap_or(0);
    format!("{mant}e{}{}", if e >= 0 { '+' } else { '-' }, e.abs())
}

/// `toPrecision(p)`.
pub fn to_precision(n: f64, p: usize) -> String {
    if !n.is_finite() {
        return number_to_string(n);
    }
    if n == 0.0 {
        return if p > 1 { format!("0.{}", "0".repeat(p - 1)) } else { "0".into() };
    }
    let s = format!("{:.*e}", p - 1, n);
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let e: i32 = exp.parse().unwrap_or(0);
    if e < -6 || e >= p as i32 {
        return format!("{mant}e{}{}", if e >= 0 { '+' } else { '-' }, e.abs());
    }
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let out = if e >= 0 {
        let int_len = e as usize + 1;
        if int_len >= digits.len() {
            digits
        } else {
            format!("{}.{}", &digits[..int_len], &digits[int_len..])
        }
    } else {
        format!("0.{}{}", "0".repeat((-e - 1) as usize), digits)
    };
    if neg { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n2s(n: f64) -> String {
        number_to_string(n)
    }

    #[test]
    fn numeros_para_texto() {
        assert_eq!(n2s(0.1), "0.1");
        assert_eq!(n2s(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(n2s(123.0), "123");
        assert_eq!(n2s(-1.5), "-1.5");
        assert_eq!(n2s(1e21), "1e+21");
        assert_eq!(n2s(1e-7), "1e-7");
        assert_eq!(n2s(1.5e-7), "1.5e-7");
        assert_eq!(n2s(0.000001), "0.000001");
        assert_eq!(n2s(123456789012345680000.0), "123456789012345680000");
        assert_eq!(n2s(f64::NAN), "NaN");
        assert_eq!(n2s(-0.0), "0");
        assert_eq!(number_to_string_radix(255.0, 16), "ff");
        assert_eq!(number_to_string_radix(0.5, 2), "0.1");
        assert_eq!(number_to_string_radix(-10.0, 2), "-1010");
    }

    #[test]
    fn texto_para_numeros() {
        let s = |x: &str| string_to_number(&x.encode_utf16().collect::<Vec<_>>());
        assert_eq!(s("  42 "), 42.0);
        assert_eq!(s("0x1F"), 31.0);
        assert_eq!(s(""), 0.0);
        assert_eq!(s("1e3"), 1000.0);
        assert_eq!(s(".5"), 0.5);
        assert_eq!(s("5."), 5.0);
        assert!(s("inf").is_nan());
        assert!(s("12px").is_nan());
        assert_eq!(s("-Infinity"), f64::NEG_INFINITY);
        let pi = |x: &str, r| parse_int(&x.encode_utf16().collect::<Vec<_>>(), r);
        assert_eq!(pi("12px", 0), 12.0);
        assert_eq!(pi("0x10", 0), 16.0);
        assert_eq!(pi("-7.9", 10), -7.0);
        assert!(pi("abc", 10).is_nan());
        let pf = |x: &str| parse_float(&x.encode_utf16().collect::<Vec<_>>());
        assert_eq!(pf("3.14abc"), 3.14);
        assert_eq!(pf("1e3x"), 1000.0);
        assert_eq!(pf("-.5"), -0.5);
    }

    #[test]
    fn formatos() {
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.25, 1), "1.3");
        assert_eq!(to_fixed(123.456, 1), "123.5");
        assert_eq!(to_fixed(-1.5, 0), "-2");
        assert_eq!(to_exponential(12345.0, Some(2)), "1.23e+4");
        assert_eq!(to_precision(123.456, 4), "123.5");
        assert_eq!(to_precision(0.00012, 2), "0.00012");
        assert_eq!(to_precision(123456.0, 2), "1.2e+5");
        assert_eq!(to_int32(4294967297.0), 1);
        assert_eq!(to_int32(-1.0), -1);
        assert_eq!(to_uint32(-1.0), 4294967295);
    }
}

/// Número → bits de um float16 (IEEE 754 binary16), arredondando para o
/// mais próximo com empate para o par, direto do f64 (sem passar por f32).
pub fn f64_to_f16(x: f64) -> u16 {
    let sign = if x.is_sign_negative() { 0x8000u16 } else { 0 };
    if x.is_nan() {
        return 0x7e00;
    }
    let a = x.abs();
    // 65520 fica no meio entre 65504 (maior finito) e 65536: empate vai para Infinity
    if a >= 65520.0 {
        return sign | 0x7c00;
    }
    if a < 6.103515625e-05 {
        // subnormal (ou zero): m × 2^-24; arredondar a 1024 vira o menor normal
        return sign | (a * 16777216.0).round_ties_even() as u16;
    }
    let e = ((a.to_bits() >> 52) & 0x7ff) as i32 - 1023;
    // mantissa com 10 bits: [1024, 2048]; 2048 sobe o expoente sozinho
    let m = (a * f64::powi(2.0, 10 - e)).round_ties_even() as u32;
    sign | ((((e + 15) as u32) << 10) + (m - 1024)) as u16
}

/// Bits de um float16 → número.
pub fn f16_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f64;
    match e {
        0 => sign * m * f64::powi(2.0, -24),
        31 if m == 0.0 => sign * f64::INFINITY,
        31 => f64::NAN,
        _ => sign * (1024.0 + m) * f64::powi(2.0, e - 25),
    }
}
