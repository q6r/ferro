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

    pub fn is_private(&self) -> bool {
        self.0.private
    }

    pub fn description(&self) -> Option<&JsStr> {
        self.0.description.as_ref()
    }
}

impl PartialEq for Sym {
    fn eq(&self, other: &Sym) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Sym {}

impl Hash for Sym {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Rc::as_ptr(&self.0) as usize).hash(state)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum PropKey {
    Str(JsStr),
    Sym(Sym),
}

impl PropKey {
    pub fn str(s: &str) -> PropKey {
        PropKey::Str(JsStr::new(s))
    }

    pub fn as_index(&self) -> Option<u32> {
        match self {
            PropKey::Str(s) => s.as_index(),
            PropKey::Sym(_) => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            PropKey::Str(s) => Value::String(s.clone()),
            PropKey::Sym(s) => Value::Symbol(s.clone()),
        }
    }
}

impl From<&str> for PropKey {
    fn from(s: &str) -> PropKey {
        PropKey::str(s)
    }
}

#[derive(Clone)]
pub enum Slot {
    Data { value: Value, writable: bool },
    Accessor { get: Option<ObjRef>, set: Option<ObjRef> },
}

#[derive(Clone)]
pub struct Property {
    pub slot: Slot,
    pub enumerable: bool,
    pub configurable: bool,
}

impl Property {
    /// Propriedade comum (gravável, enumerável, configurável).
    pub fn data(value: Value) -> Property {
        Property {
            slot: Slot::Data { value, writable: true },
            enumerable: true,
            configurable: true,
        }
    }

    /// Propriedade não enumerável (métodos dos embutidos).
    pub fn hidden(value: Value) -> Property {
        Property {
            slot: Slot::Data { value, writable: true },
            enumerable: false,
            configurable: true,
        }
    }
}

/// Propriedades em ordem de inserção. Objetos pequenos (a maioria) são
/// buscados por varredura linear; o índice por hash só aparece quando o
/// objeto passa de `LINEAR` propriedades.
#[derive(Default, Clone)]
pub struct PropMap {
    entries: Vec<(PropKey, Property)>,
    index: Option<Box<crate::fxhash::FxHashMap<PropKey, usize>>>,
    /// Muda quando as posições das entradas mudam (remoção): invalida caches.
    generation: u32,
}

const LINEAR: usize = 8;

#[inline]
fn key_eq(a: &PropKey, b: &PropKey) -> bool {
    match (a, b) {
        (PropKey::Str(x), PropKey::Str(y)) => x == y,
        (PropKey::Sym(x), PropKey::Sym(y)) => x == y,
        _ => false,
    }
}

impl PropMap {
    #[inline]
    fn find(&self, k: &PropKey) -> Option<usize> {
        match &self.index {
            Some(ix) => ix.get(k).copied(),
            None => self.entries.iter().position(|(e, _)| key_eq(e, k)),
        }
    }

    fn rebuild_index(&mut self) {
        if self.entries.len() > LINEAR {
            let mut ix = crate::fxhash::FxHashMap::default();
            ix.reserve(self.entries.len());
            for (i, (k, _)) in self.entries.iter().enumerate() {
                ix.insert(k.clone(), i);
            }
            self.index = Some(Box::new(ix));
        } else {
            self.index = None;
        }
    }

    pub fn get(&self, k: &PropKey) -> Option<&Property> {
        self.find(k).map(|i| &self.entries[i].1)
    }

    pub fn get_mut(&mut self, k: &PropKey) -> Option<&mut Property> {
        match self.find(k) {
            Some(i) => Some(&mut self.entries[i].1),
            None => None,
        }
    }

