//! Valores, objetos, propriedades e ambientes (escopos em tempo de execução).
//!
//! Por enquanto a memória é gerenciada com contagem de referências; ciclos
//! vazam até o coletor de lixo do M3.5.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::compiler::Code;
use crate::string::JsStr;
use crate::vm::Vm;

#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(JsStr),
    Symbol(Sym),
    Object(ObjRef),
    BigInt(Rc<crate::bigint::BigInt>),
    /// Interno: variável ainda não inicializada (TDZ) ou buraco de array.
    Empty,
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::String(JsStr::new(s))
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, Value::Undefined | Value::Null)
    }

    pub fn as_object(&self) -> Option<&ObjRef> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn is_callable(&self) -> bool {
        self.as_object().is_some_and(|o| o.is_callable())
    }
}

/// Referência a um objeto (igualdade por identidade).
#[derive(Clone)]
pub struct ObjRef(pub Rc<RefCell<Object>>);

impl ObjRef {
