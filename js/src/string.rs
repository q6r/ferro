//! Strings do JavaScript: sequências de unidades UTF-16 (é o que a
//! linguagem exige: `"ç".length === 1`, `"😀".length === 2`).
//!
//! Concatenações grandes viram "cordas" (ropes): um nó com as duas metades,
//! achatado só quando alguém precisa do conteúdo. Assim `s += x` em laço
//! fica linear em vez de quadrático.

use std::cell::{OnceCell, RefCell};
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// Abaixo disso, concatenar copia direto (mais barato que criar um nó).
const ROPE_MIN: usize = 256;

#[derive(Clone)]
pub struct JsStr(Rc<StrData>);

struct StrData {
    len: usize,
    /// Hash já calculado (0 = ainda não): strings são imutáveis.
    hash: std::cell::Cell<u64>,
    flat: OnceCell<Box<[u16]>>,
    parts: RefCell<Option<(JsStr, JsStr)>>,
}

impl Drop for StrData {
    /// Solta cordas profundas sem recursão (evita estourar a pilha).
    fn drop(&mut self) {
        let Some((a, b)) = self.parts.get_mut().take() else { return };
        let mut stack = vec![a, b];
        while let Some(s) = stack.pop() {
            if let Ok(mut data) = Rc::try_unwrap(s.0)
                && let Some((x, y)) = data.parts.get_mut().take()
            {
                stack.push(x);
                stack.push(y);
            }
        }
    }
}

impl JsStr {
    fn from_box(b: Box<[u16]>) -> JsStr {
        let len = b.len();
        let flat = OnceCell::new();
        let _ = flat.set(b);
        JsStr(Rc::new(StrData {
            hash: std::cell::Cell::new(0),
            len,
            flat,
            parts: RefCell::new(None),
        }))
    }

    pub fn new(s: &str) -> JsStr {
        JsStr::from_box(s.encode_utf16().collect::<Vec<u16>>().into_boxed_slice())
    }

    pub fn from_units(v: Vec<u16>) -> JsStr {
        JsStr::from_box(v.into_boxed_slice())
    }

    pub fn from_slice(v: &[u16]) -> JsStr {
        JsStr::from_box(v.into())
    }

    pub fn empty() -> JsStr {
        JsStr::from_box(Box::new([]))
    }

    /// O conteúdo (achata a corda na primeira vez).
    pub fn units(&self) -> &[u16] {
        self.0.flat.get_or_init(|| self.flatten())
    }

    fn flatten(&self) -> Box<[u16]> {
        let mut out: Vec<u16> = Vec::with_capacity(self.0.len);
        let root = self.0.parts.borrow().clone();
        let mut stack: Vec<JsStr> = Vec::new();
        if let Some((a, b)) = root {
            stack.push(b);
            stack.push(a);
        }
        while let Some(s) = stack.pop() {
            if let Some(f) = s.0.flat.get() {
                out.extend_from_slice(f);
                continue;
            }
            let parts = s.0.parts.borrow().clone();
            if let Some((a, b)) = parts {
                stack.push(b);
                stack.push(a);
            }
        }
        // as metades não são mais necessárias
        let old = self.0.parts.borrow_mut().take();
        drop(stack);
        drop(old);
        out.into_boxed_slice()
    }

    pub fn len(&self) -> usize {
        self.0.len
    }

    pub fn is_empty(&self) -> bool {
        self.0.len == 0
    }

    pub fn to_rust(&self) -> String {
        String::from_utf16_lossy(self.units())
    }

    pub fn concat(&self, other: &JsStr) -> JsStr {
        if other.is_empty() {
            return self.clone();
        }
        if self.is_empty() {
            return other.clone();
        }
        let len = self.len() + other.len();
        if len < ROPE_MIN {
            let mut v = Vec::with_capacity(len);
            v.extend_from_slice(self.units());
            v.extend_from_slice(other.units());
            return JsStr::from_units(v);
        }
        JsStr(Rc::new(StrData {
            hash: std::cell::Cell::new(0),
            len,
            flat: OnceCell::new(),
            parts: RefCell::new(Some((self.clone(), other.clone()))),
        }))
    }

    pub fn slice(&self, start: usize, end: usize) -> JsStr {
        let end = end.min(self.len());
        let start = start.min(end);
        JsStr::from_slice(&self.units()[start..end])
    }

    /// Índice canônico de array ("0", "1", ... sem zeros à esquerda).
    pub fn as_index(&self) -> Option<u32> {
        if self.0.len == 0 || self.0.len > 10 {
            return None;
        }
        let u = self.units();
        if u.len() > 1 && u[0] == b'0' as u16 {
            return None;
        }
        let mut n: u64 = 0;
        for &c in u.iter() {
            if !(b'0' as u16..=b'9' as u16).contains(&c) {
                return None;
            }
            n = n * 10 + (c - b'0' as u16) as u64;
        }
        if n < u32::MAX as u64 { Some(n as u32) } else { None }
    }

    pub fn find(&self, needle: &JsStr, from: usize) -> Option<usize> {
        let (h, n) = (self.units(), needle.units());
        if n.is_empty() {
            return Some(from.min(h.len()));
        }
        if n.len() > h.len() || from > h.len() - n.len() {
            return None;
        }
        let first = n[0];
        let mut i = from;
        while i <= h.len() - n.len() {
            match h[i..=h.len() - n.len()].iter().position(|&c| c == first) {
                Some(p) => i += p,
                None => return None,
            }
            if &h[i..i + n.len()] == n {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    pub fn rfind(&self, needle: &JsStr, from: usize) -> Option<usize> {
        let (h, n) = (self.units(), needle.units());
        if n.len() > h.len() {
            return None;
        }
        let start = from.min(h.len() - n.len());
        (0..=start).rev().find(|&i| &h[i..i + n.len()] == n)
    }

    pub fn eq_str(&self, s: &str) -> bool {
        if self.0.len > s.len() {
            return false;
        }
        self.units().iter().copied().eq(s.encode_utf16())
    }
}

impl PartialEq for JsStr {
    fn eq(&self, other: &JsStr) -> bool {
        Rc::ptr_eq(&self.0, &other.0) || (self.0.len == other.0.len && self.units() == other.units())
    }
}

impl Eq for JsStr {}

impl Hash for JsStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut h = self.0.hash.get();
        if h == 0 {
            let mut fx = crate::fxhash::FxHasher::default();
            self.units().hash(&mut fx);
            h = fx.finish().max(1);
            self.0.hash.set(h);
        }
        state.write_u64(h);
    }
}

impl PartialOrd for JsStr {
    fn partial_cmp(&self, other: &JsStr) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for JsStr {
    fn cmp(&self, other: &JsStr) -> Ordering {
        self.units().cmp(other.units())
    }
}

impl From<&str> for JsStr {
    fn from(s: &str) -> JsStr {
        JsStr::new(s)
    }
}

impl From<String> for JsStr {
    fn from(s: String) -> JsStr {
        JsStr::new(&s)
    }
}

