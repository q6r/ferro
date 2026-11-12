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

