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
        if self.mag.is_empty() {
            return 0.0;
        }
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

    pub fn to_string_radix(&self, radix: u32) -> String {
        if self.mag.is_empty() {
            return "0".into();
        }
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        // divide por radix^k de uma vez (k dígitos por pedaço)
        let mut k = 1u32;
        let mut chunk = radix as u64;
        while chunk * (radix as u64) < (1 << 32) {
            chunk *= radix as u64;
            k += 1;
        }
        let mut parts: Vec<u32> = Vec::new();
        let mut cur = self.mag.clone();
        while !cur.is_empty() {
            let (q, r) = mag_divrem_small(&cur, chunk as u32);
            parts.push(r);
            cur = q;
        }
        let mut out = String::new();
        if self.neg {
            out.push('-');
        }
        for (i, &p) in parts.iter().rev().enumerate() {
            let mut buf = Vec::new();
            let mut x = p;
            for _ in 0..k {
                buf.push(digits[(x % radix) as usize]);
                x /= radix;
            }
            while i == 0 && buf.len() > 1 && *buf.last().unwrap() == b'0' {
                buf.pop();
            }
            buf.reverse();
            out.push_str(std::str::from_utf8(&buf).unwrap_or(""));
        }
        out
    }

    pub fn neg(&self) -> BigInt {
        BigInt::from_parts(!self.neg, self.mag.clone())
    }

    pub fn add(&self, o: &BigInt) -> BigInt {
        if self.neg == o.neg {
            return BigInt::from_parts(self.neg, mag_add(&self.mag, &o.mag));
        }
        match mag_cmp(&self.mag, &o.mag) {
            Ordering::Equal => BigInt::zero(),
            Ordering::Greater => BigInt::from_parts(self.neg, mag_sub(&self.mag, &o.mag)),
            Ordering::Less => BigInt::from_parts(o.neg, mag_sub(&o.mag, &self.mag)),
        }
    }

    pub fn sub(&self, o: &BigInt) -> BigInt {
        self.add(&o.neg())
    }

    pub fn mul(&self, o: &BigInt) -> BigInt {
        BigInt::from_parts(self.neg != o.neg, mag_mul(&self.mag, &o.mag))
    }

    /// Divisão truncada (como no JS); None se o divisor for zero.
    pub fn div(&self, o: &BigInt) -> Option<BigInt> {
        if o.is_zero() {
            return None;
        }
        let (q, _) = mag_divrem(&self.mag, &o.mag);
        Some(BigInt::from_parts(self.neg != o.neg, q))
    }

    /// Resto com o sinal do dividendo; None se o divisor for zero.
    pub fn rem(&self, o: &BigInt) -> Option<BigInt> {
        if o.is_zero() {
            return None;
        }
        let (_, r) = mag_divrem(&self.mag, &o.mag);
        Some(BigInt::from_parts(self.neg, r))
    }

    pub fn pow(&self, mut e: u64) -> BigInt {
        let mut base = self.clone();
        let mut acc = BigInt::from_i64(1);
        while e > 0 {
            if e & 1 == 1 {
                acc = acc.mul(&base);
            }
            e >>= 1;
            if e > 0 {
                base = base.mul(&base);
            }
        }
        acc
    }

    pub fn shl(&self, n: u64) -> BigInt {
        if self.is_zero() {
            return self.clone();
        }
        let limbs = (n / 32) as usize;
        let bits = (n % 32) as u32;
        let mut mag = vec![0u32; limbs];
        mag.extend(shl_bits(&self.mag, bits, true));
        BigInt::from_parts(self.neg, mag)
    }

    /// Desloca a magnitude para a direita (sem tratar o sinal).
    fn shr_mag(&self, n: u64) -> BigInt {
        let limbs = (n / 32) as usize;
        if limbs >= self.mag.len() {
            return BigInt::zero();
        }
        let bits = (n % 32) as u32;
        let src = &self.mag[limbs..];
        let mut mag = Vec::with_capacity(src.len());
        for i in 0..src.len() {
            let lo = src[i] >> bits;
            let hi = if bits > 0 { src.get(i + 1).map_or(0, |x| x << (32 - bits)) } else { 0 };
            mag.push(lo | hi);
        }
        BigInt::from_parts(self.neg, mag)
    }

    /// `>>` aritmético (arredonda para baixo, como em complemento de dois).
    pub fn shr(&self, n: u64) -> BigInt {
        if !self.neg {
            return self.shr_mag(n);
        }
        // -((|x| - 1) >> n) - 1
        let one = BigInt::from_i64(1);
        let m = self.clone_abs().sub(&one).shr_mag(n);
        m.add(&one).neg()
    }

    /// Representação em complemento de dois com `n` dígitos.
    fn twos(&self, n: usize) -> Vec<u32> {
        let mut v = self.mag.clone();
        v.resize(n, 0);
        if self.neg {
            for x in v.iter_mut() {
                *x = !*x;
            }
            for x in v.iter_mut() {
                let (s, c) = x.overflowing_add(1);
                *x = s;
                if !c {
                    break;
                }
            }
        }
        v
    }

    fn from_twos(mut v: Vec<u32>) -> BigInt {
        let neg = v.last().is_some_and(|x| x >> 31 == 1);
        if neg {
            for x in v.iter_mut() {
                *x = !*x;
            }
            for x in v.iter_mut() {
                let (s, c) = x.overflowing_add(1);
                *x = s;
                if !c {
                    break;
                }
            }
        }
        BigInt::from_parts(neg, v)
    }

    fn bitop(&self, o: &BigInt, f: impl Fn(u32, u32) -> u32) -> BigInt {
        let n = self.mag.len().max(o.mag.len()) + 1;
        let (a, b) = (self.twos(n), o.twos(n));
        BigInt::from_twos(a.iter().zip(b.iter()).map(|(x, y)| f(*x, *y)).collect())
    }

    pub fn and(&self, o: &BigInt) -> BigInt {
        self.bitop(o, |x, y| x & y)
    }

    pub fn or(&self, o: &BigInt) -> BigInt {
        self.bitop(o, |x, y| x | y)
    }

    pub fn xor(&self, o: &BigInt) -> BigInt {
        self.bitop(o, |x, y| x ^ y)
    }

    pub fn not(&self) -> BigInt {
        self.neg().sub(&BigInt::from_i64(1))
    }

    pub fn cmp(&self, o: &BigInt) -> Ordering {
        match (self.neg, o.neg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => mag_cmp(&self.mag, &o.mag),
            (true, true) => mag_cmp(&o.mag, &self.mag),
        }
    }

    /// Comparação exata com um Number (None se for NaN).
    pub fn cmp_f64(&self, f: f64) -> Option<Ordering> {
        if f.is_nan() {
            return None;
        }
        if f == f64::INFINITY {
            return Some(Ordering::Less);
        }
        if f == f64::NEG_INFINITY {
            return Some(Ordering::Greater);
        }
        let fl = f.floor();
        let b = BigInt::from_f64(fl)?;
        match self.cmp(&b) {
            Ordering::Equal if f > fl => Some(Ordering::Less),
            o => Some(o),
        }
    }

    /// BigInt.asUintN(bits, x).
    pub fn as_uint_n(&self, bits: u64) -> BigInt {
        if bits == 0 {
            return BigInt::zero();
        }
        let limbs = bits.div_ceil(32) as usize;
        let mut v = self.twos(limbs.max(self.mag.len() + 1));
        v.truncate(limbs);
        let extra = (limbs as u64) * 32 - bits;
        if extra > 0
            && let Some(top) = v.last_mut()
        {
            *top &= u32::MAX >> extra;
        }
        BigInt::from_parts(false, v)
    }

    /// BigInt.asIntN(bits, x).
    pub fn as_int_n(&self, bits: u64) -> BigInt {
        if bits == 0 {
            return BigInt::zero();
        }
        let u = self.as_uint_n(bits);
        let half = BigInt::from_i64(1).shl(bits - 1);
        if u.cmp(&half) != Ordering::Less { u.sub(&half.shl(1)) } else { u }
    }

    /// Valor como i64/u64 truncado em 64 bits (para typed arrays).
    pub fn to_u64_wrapping(&self) -> u64 {
        let v = self.as_uint_n(64);
        let mut x = 0u64;
        for (i, &d) in v.mag.iter().enumerate().take(2) {
            x |= (d as u64) << (32 * i);
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> BigInt {
        BigInt::parse_js(s).unwrap()
    }

    #[test]
    fn aritmetica() {
        let x = b("123456789012345678901234567890");
        let y = b("987654321098765432109876543210");
        assert_eq!(x.add(&y).to_string_radix(10), "1111111110111111111011111111100");
        assert_eq!(x.sub(&y).to_string_radix(10), "-864197532086419753208641975320");
        assert_eq!(x.mul(&y).to_string_radix(10), "121932631137021795226185032733622923332237463801111263526900");
        assert_eq!(y.div(&x).unwrap().to_string_radix(10), "8");
        assert_eq!(y.rem(&x).unwrap().to_string_radix(10), "9000000000900000000090");
        assert_eq!(b("-7").div(&b("2")).unwrap().to_string_radix(10), "-3");
        assert_eq!(b("-7").rem(&b("2")).unwrap().to_string_radix(10), "-1");
        assert_eq!(b("2").pow(100).to_string_radix(10), "1267650600228229401496703205376");
        assert_eq!(b("255").to_string_radix(16), "ff");
        assert_eq!(b("-1").shl(70).to_string_radix(10), "-1180591620717411303424");
        assert_eq!(b("-9").shr(1).to_string_radix(10), "-5");
        assert_eq!(b("-6").and(&b("3")).to_string_radix(10), "2");
        assert_eq!(b("-6").or(&b("3")).to_string_radix(10), "-5");
        assert_eq!(b("5").not().to_string_radix(10), "-6");
        assert_eq!(b("255").as_int_n(8).to_string_radix(10), "-1");
        assert_eq!(b("-1").as_uint_n(64).to_string_radix(10), "18446744073709551615");
        assert_eq!(BigInt::from_f64(1e20).unwrap().to_string_radix(10), "100000000000000000000");
        assert_eq!(b("9007199254740993").to_f64(), 9007199254740992.0);
        assert_eq!(b("123456789012345678901234567890").to_f64(), 1.2345678901234568e29);
        assert_eq!(BigInt::parse_literal("0xFF_FFn").unwrap().to_string_radix(10), "65535");
        // divisão longa com vários dígitos
        let big = b("340282366920938463463374607431768211457");
        let d = b("18446744073709551617");
        assert_eq!(big.div(&d).unwrap().to_string_radix(10), "18446744073709551615");
        assert_eq!(big.rem(&d).unwrap().to_string_radix(10), "2");
    }
}
