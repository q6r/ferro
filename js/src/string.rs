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

