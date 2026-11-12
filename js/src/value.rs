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
    pub fn new(obj: Object) -> ObjRef {
        let rc = Rc::new(RefCell::new(obj));
        crate::gc::track_object(&rc);
        ObjRef(rc)
    }

    pub fn ptr_eq(&self, other: &ObjRef) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    pub fn borrow(&self) -> std::cell::Ref<'_, Object> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, Object> {
        self.0.borrow_mut()
    }

    pub fn id(&self) -> usize {
        Rc::as_ptr(&self.0) as *const u8 as usize
    }

    pub fn is_callable(&self) -> bool {
        match &self.borrow().kind {
            Kind::Function(_) => true,
            Kind::Proxy { target: Some(t), .. } => t.is_callable(),
            _ => false,
        }
    }

    pub fn is_constructor(&self) -> bool {
        match &self.borrow().kind {
            Kind::Function(Func::Closure(c)) => c.code.is_constructor(),
            Kind::Function(Func::Native(n)) => n.ctor,
            Kind::Function(Func::Bound { target, .. }) => target.is_constructor(),
            Kind::Proxy { target: Some(t), .. } => t.is_constructor(),
            _ => false,
        }
    }

    pub fn proto(&self) -> Option<ObjRef> {
        self.borrow().proto.clone()
    }
}

impl PartialEq for ObjRef {
    fn eq(&self, other: &ObjRef) -> bool {
        self.ptr_eq(other)
    }
}

impl Eq for ObjRef {}

impl Hash for ObjRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id().hash(state)
    }
}

pub struct SymData {
    pub description: Option<JsStr>,
    /// Nome privado de classe (`#x`): invisível para reflexão.
    pub private: bool,
}

#[derive(Clone)]
pub struct Sym(pub Rc<SymData>);

impl Sym {
    pub fn new(description: Option<JsStr>) -> Sym {
        Sym(Rc::new(SymData {
            description,
            private: false,
        }))
    }

    pub fn new_private(description: JsStr) -> Sym {
        Sym(Rc::new(SymData {
            description: Some(description),
            private: true,
        }))
    }

