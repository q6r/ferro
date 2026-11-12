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

