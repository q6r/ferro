//! BigInt: inteiros de precisão arbitrária (sinal + magnitude em base 2^32).

use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct BigInt {
    neg: bool,
    /// Dígitos em base 2^32, do menos para o mais significativo, sem zeros no topo.
    mag: Vec<u32>,
}

fn trim(v: &mut Vec<u32>) {
    while v.last() == Some(&0) {
        v.pop();
    }
}

fn mag_cmp(a: &[u32], b: &[u32]) -> Ordering {
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    Ordering::Equal
}

fn mag_add(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (a, b) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = Vec::with_capacity(a.len() + 1);
    let mut carry = 0u64;
    for i in 0..a.len() {
        let s = a[i] as u64 + *b.get(i).unwrap_or(&0) as u64 + carry;
        out.push(s as u32);
        carry = s >> 32;
    }
    if carry > 0 {
        out.push(carry as u32);
    }
    out
}

/// a - b, com a >= b.
fn mag_sub(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0i64;
    for i in 0..a.len() {
        let mut d = a[i] as i64 - *b.get(i).unwrap_or(&0) as i64 - borrow;
        if d < 0 {
            d += 1 << 32;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out.push(d as u32);
    }
    trim(&mut out);
    out
}

fn mag_mul(a: &[u32], b: &[u32]) -> Vec<u32> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u32; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &y) in b.iter().enumerate() {
            let t = out[i + j] as u64 + x as u64 * y as u64 + carry;
            out[i + j] = t as u32;
            carry = t >> 32;
        }
        let mut k = i + b.len();
        while carry > 0 {
            let t = out[k] as u64 + carry;
            out[k] = t as u32;
            carry = t >> 32;
            k += 1;
        }
    }
    trim(&mut out);
    out
}

fn mag_divrem_small(a: &[u32], d: u32) -> (Vec<u32>, u32) {
    let mut q = vec![0u32; a.len()];
    let mut r = 0u64;
    for i in (0..a.len()).rev() {
        let cur = (r << 32) | a[i] as u64;
        q[i] = (cur / d as u64) as u32;
        r = cur % d as u64;
    }
    trim(&mut q);
    (q, r as u32)
}

fn shl_bits(v: &[u32], s: u32, extra: bool) -> Vec<u32> {
    let mut out = Vec::with_capacity(v.len() + 1);
    if s == 0 {
        out.extend_from_slice(v);
        if extra {
            out.push(0);
        }
        return out;
    }
    let mut carry = 0u32;
    for &x in v {
        out.push((x << s) | carry);
        carry = x >> (32 - s);
    }
    if extra {
        out.push(carry);
    }
    out
}

/// Divisão longa (algoritmo D de Knuth, como no Hacker's Delight).
fn mag_divrem(u: &[u32], v: &[u32]) -> (Vec<u32>, Vec<u32>) {
    if mag_cmp(u, v) == Ordering::Less {
        return (Vec::new(), u.to_vec());
    }
    if v.len() == 1 {
        let (q, r) = mag_divrem_small(u, v[0]);
        return (q, if r == 0 { Vec::new() } else { vec![r] });
    }
    let n = v.len();
    let m = u.len() - n;
    let s = v[n - 1].leading_zeros();
    let vn = shl_bits(v, s, false);
    let mut un = shl_bits(u, s, true);
    let mut q = vec![0u32; m + 1];
    let b: u64 = 1 << 32;
    for j in (0..=m).rev() {
        let num = ((un[j + n] as u64) << 32) | un[j + n - 1] as u64;
        let mut qhat = num / vn[n - 1] as u64;
        let mut rhat = num % vn[n - 1] as u64;
        while qhat >= b || (qhat as u128) * (vn[n - 2] as u128) > ((rhat as u128) << 32) + un[j + n - 2] as u128 {
            qhat -= 1;
            rhat += vn[n - 1] as u64;
            if rhat >= b {
                break;
            }
        }
        let mut k: i64 = 0;
        for i in 0..n {
            let p: u64 = qhat * vn[i] as u64;
            let t: i64 = un[i + j] as i64 - k - (p & 0xFFFF_FFFF) as i64;
            un[i + j] = t as u32;
            k = (p >> 32) as i64 - (t >> 32);
        }
        let t: i64 = un[j + n] as i64 - k;
        un[j + n] = t as u32;
        if t < 0 {
            qhat -= 1;
            let mut c = 0u64;
            for i in 0..n {
                let sum = un[i + j] as u64 + vn[i] as u64 + c;
                un[i + j] = sum as u32;
                c = sum >> 32;
            }
            un[j + n] = un[j + n].wrapping_add(c as u32);
        }
        q[j] = qhat as u32;
    }
    trim(&mut q);
    // resto = un desnormalizado
    let mut r = vec![0u32; n];
    for i in 0..n {
        r[i] = if s == 0 { un[i] } else { (un[i] >> s) | (un[i + 1] << (32 - s)) };
    }
    trim(&mut r);
    (q, r)
}

impl BigInt {
    pub fn zero() -> BigInt {
        BigInt::default()
    }

    fn from_parts(neg: bool, mut mag: Vec<u32>) -> BigInt {
        trim(&mut mag);
        let neg = neg && !mag.is_empty();
        BigInt { neg, mag }
    }

    pub fn from_i64(v: i64) -> BigInt {
        let neg = v < 0;
        let m = v.unsigned_abs();
        BigInt::from_parts(neg, vec![m as u32, (m >> 32) as u32])
    }

    pub fn from_u64(v: u64) -> BigInt {
        BigInt::from_parts(false, vec![v as u32, (v >> 32) as u32])
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    pub fn is_negative(&self) -> bool {
        self.neg
    }

    /// Número inteiro exato → BigInt (None se não for inteiro).
    pub fn from_f64(f: f64) -> Option<BigInt> {
        if !f.is_finite() || f.fract() != 0.0 {
            return None;
        }
        if f == 0.0 {
            return Some(BigInt::zero());
        }
        let bits = f.to_bits();
        let neg = (bits >> 63) != 0;
        let exp = ((bits >> 52) & 0x7FF) as i64;
        let mant = if exp == 0 { (bits & ((1 << 52) - 1)) << 1 } else { (bits & ((1 << 52) - 1)) | (1 << 52) };
        let shift = exp - 1075;
        let base = BigInt::from_u64(mant);
        let v = if shift >= 0 { base.shl(shift as u64) } else { base.shr_mag((-shift) as u64) };
        Some(if neg { v.neg() } else { v })
    }

    /// Conversão para Number com arredondamento correto.
    pub fn to_f64(&self) -> f64 {
        let bits = self.mag.len() * 32 - self.mag.last().map_or(32, |x| x.leading_zeros() as usize);
        let v = if bits <= 64 {
            let mut x = 0u64;
            for (i, &d) in self.mag.iter().enumerate() {
                x |= (d as u64) << (32 * i);
            }
            x as f64
        } else {
            if bits > 1100 {
                return if self.neg { f64::NEG_INFINITY } else { f64::INFINITY };
            }
            // os 64 bits do topo + "bit grudento" do resto
            let shift = bits - 64;
            let top = self.clone_abs().shr_mag(shift as u64);
            let mut x = 0u64;
            for (i, &d) in top.mag.iter().enumerate().take(2) {
                x |= (d as u64) << (32 * i);
            }
            let lost = self.clone_abs().sub(&top.shl(shift as u64));
            if !lost.is_zero() {
                x |= 1;
            }
            x as f64 * 2f64.powi(shift as i32)
        };
        if self.neg { -v } else { v }
    }

    fn clone_abs(&self) -> BigInt {
        BigInt {
            neg: false,
            mag: self.mag.clone(),
        }
    }

    /// Texto do literal (`123n`, `0xFFn`...), sem o `n` e sem `_`.
    pub fn parse_literal(text: &str) -> Option<BigInt> {
        let t: String = text.chars().filter(|&c| c != '_' && c != 'n').collect();
        let lower = t.to_ascii_lowercase();
        let (digits, radix) = if let Some(r) = lower.strip_prefix("0x") {
            (r.to_string(), 16)
        } else if let Some(r) = lower.strip_prefix("0o") {
            (r.to_string(), 8)
        } else if let Some(r) = lower.strip_prefix("0b") {
            (r.to_string(), 2)
        } else {
            (lower, 10)
        };
        BigInt::parse_radix(&digits, radix)
    }

    pub fn parse_radix(digits: &str, radix: u32) -> Option<BigInt> {
        if digits.is_empty() {
            return None;
        }
        let mut mag: Vec<u32> = Vec::new();
        for c in digits.chars() {
            let d = c.to_digit(radix)?;
            // mag = mag * radix + d
            let mut carry = d as u64;
            for x in mag.iter_mut() {
                let t = *x as u64 * radix as u64 + carry;
                *x = t as u32;
                carry = t >> 32;
            }
            if carry > 0 {
                mag.push(carry as u32);
            }
        }
        Some(BigInt::from_parts(false, mag))
    }

    /// StringToBigInt (para `BigInt("...")` e comparações com strings).
    pub fn parse_js(s: &str) -> Option<BigInt> {
        let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
        if t.is_empty() {
            return Some(BigInt::zero());
        }
        let lower = t.to_ascii_lowercase();
        for (p, r) in [("0x", 16), ("0o", 8), ("0b", 2)] {
            if let Some(rest) = lower.strip_prefix(p) {
                return BigInt::parse_radix(rest, r);
            }
        }
        let (neg, digits) = match t.as_bytes()[0] {
            b'-' => (true, &t[1..]),
            b'+' => (false, &t[1..]),
            _ => (false, t),
        };
        let v = BigInt::parse_radix(digits, 10)?;
        Some(if neg { v.neg() } else { v })
    }

