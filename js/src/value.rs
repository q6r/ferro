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

    /// CanonicalNumericIndexString: o número, se a chave for a forma
    /// canônica de um número ("1", "-0", "1.5", "NaN"...).
    pub fn numeric_index(&self) -> Option<f64> {
        let PropKey::Str(s) = self else { return None };
        if let Some(i) = s.as_index() {
            return Some(i as f64);
        }
        let u = s.units();
        let c = *u.first()?;
        if !(c == b'-' as u16 || c == b'I' as u16 || c == b'N' as u16 || (b'0' as u16..=b'9' as u16).contains(&c)) {
            return None;
        }
        if s.eq_str("-0") {
            return Some(-0.0);
        }
        let n = crate::conv::string_to_number(u);
        s.eq_str(&crate::conv::number_to_string(n)).then_some(n)
    }
}

/// Índice inteiro não negativo (sem -0) de um número, se houver.
#[inline]
pub fn integer_index(n: f64) -> Option<usize> {
    if n >= 0.0 && n.fract() == 0.0 && !(n == 0.0 && n.is_sign_negative()) && n < 9007199254740992.0 {
        Some(n as usize)
    } else {
        None
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

    pub fn insert(&mut self, k: PropKey, p: Property) {
        match self.find(&k) {
            Some(i) => self.entries[i].1 = p,
            None => {
                let i = self.entries.len();
                match &mut self.index {
                    Some(ix) => {
                        ix.insert(k.clone(), i);
                        self.entries.push((k, p));
                    }
                    None => {
                        self.entries.push((k, p));
                        if self.entries.len() > LINEAR {
                            self.rebuild_index();
                        }
                    }
                }
            }
        }
    }

    /// Posição da chave (para caches que guardam a posição).
    pub fn position(&self, k: &PropKey) -> Option<usize> {
        self.find(k)
    }

    pub fn entry_at(&self, i: usize) -> Option<&(PropKey, Property)> {
        self.entries.get(i)
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn remove(&mut self, k: &PropKey) -> bool {
        let Some(i) = self.find(k) else { return false };
        self.generation = self.generation.wrapping_add(1);
        self.entries.remove(i);
        if self.index.is_some() {
            self.rebuild_index();
        }
        true
    }

    pub fn contains(&self, k: &PropKey) -> bool {
        self.find(k).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(PropKey, Property)> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Chaves na ordem do spec: índices inteiros crescentes, depois strings
    /// na ordem de inserção, depois símbolos.
    pub fn ordered_keys(&self) -> Vec<PropKey> {
        let mut ints: Vec<(u32, PropKey)> = Vec::new();
        let mut strs = Vec::new();
        let mut syms = Vec::new();
        for (k, _) in &self.entries {
            match k {
                PropKey::Str(s) => match s.as_index() {
                    Some(i) => ints.push((i, k.clone())),
                    None => strs.push(k.clone()),
                },
                PropKey::Sym(_) => syms.push(k.clone()),
            }
        }
        ints.sort_by_key(|(i, _)| *i);
        ints.into_iter().map(|(_, k)| k).chain(strs).chain(syms).collect()
    }
}

pub type NativeFn = fn(&mut Vm, &mut CallArgs) -> Result<Value, Value>;

/// O que uma função nativa recebe.
pub struct CallArgs {
    pub this: Value,
    pub args: Vec<Value>,
    /// Some(...) quando chamada com `new`.
    pub new_target: Option<ObjRef>,
    pub callee: ObjRef,
}

impl CallArgs {
    pub fn arg(&self, i: usize) -> Value {
        self.args.get(i).cloned().unwrap_or(Value::Undefined)
    }
}

pub struct Native {
    pub f: NativeFn,
    pub ctor: bool,
    /// Dados extras (ex.: funções resolve/reject de uma Promise).
    pub data: Value,
}

#[derive(Clone)]
pub struct Closure {
    pub code: Rc<Code>,
    pub env: Option<Env>,
    /// Arrow: o `this` e o `new.target` de onde foi criada.
    pub this: Option<Value>,
    pub new_target: Option<Value>,
    /// Objeto "casa" do método (para `super`).
    pub home: Option<ObjRef>,
    /// Classe: função que inicializa os campos da instância.
    pub fields: Option<ObjRef>,
    /// Uso do coletor de ciclos.
    pub gc: std::cell::Cell<u64>,
}

pub enum Func {
    Closure(Rc<Closure>),
    Native(Native),
    Bound { target: ObjRef, this: Value, args: Vec<Value> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IterKind {
    Keys,
    Values,
    Entries,
}

/// Chave de Map/Set (igualdade SameValueZero).
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum MapKey {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    String(JsStr),
    Symbol(Sym),
    Object(ObjRef),
    BigInt(crate::bigint::BigInt),
}

impl MapKey {
    pub fn of(v: &Value) -> MapKey {
        match v {
            Value::Undefined | Value::Empty => MapKey::Undefined,
            Value::Null => MapKey::Null,
            Value::Bool(b) => MapKey::Bool(*b),
            Value::Number(n) => {
                let n = if *n == 0.0 { 0.0 } else { *n };
                MapKey::Number(if n.is_nan() { f64::NAN.to_bits() } else { n.to_bits() })
            }
            Value::String(s) => MapKey::String(s.clone()),
            Value::Symbol(s) => MapKey::Symbol(s.clone()),
            Value::Object(o) => MapKey::Object(o.clone()),
            Value::BigInt(b) => MapKey::BigInt((**b).clone()),
        }
    }
}

/// Dados de Map e Set: entradas em ordem de inserção (com buracos ao apagar,
/// para os iteradores em andamento continuarem certos).
/// Chave fraca: objeto (sem segurar) ou símbolo (seguro; símbolos não somem).
#[derive(Clone)]
pub enum WeakKey {
    Obj(std::rc::Weak<RefCell<Object>>),
    Sym(Sym),
}

impl WeakKey {
    pub fn of(v: &Value) -> Option<(usize, WeakKey)> {
        match v {
            Value::Object(o) => Some((o.id(), WeakKey::Obj(Rc::downgrade(&o.0)))),
            Value::Symbol(s) => Some((Rc::as_ptr(&s.0) as *const u8 as usize, WeakKey::Sym(s.clone()))),
            _ => None,
        }
    }

    pub fn alive(&self) -> bool {
        match self {
            WeakKey::Obj(w) => w.strong_count() > 0,
            WeakKey::Sym(_) => true,
        }
    }

    pub fn get(&self) -> Option<Value> {
        match self {
            WeakKey::Obj(w) => w.upgrade().map(|o| Value::Object(ObjRef(o))),
            WeakKey::Sym(s) => Some(Value::Symbol(s.clone())),
        }
    }
}

/// Entradas de WeakMap/WeakSet, pelo endereço da chave. A chave morta não
/// reaparece (o endereço só é reaproveitado depois que a entrada sai).
#[derive(Default)]
pub struct WeakColl {
    pub map: crate::fxhash::FxHashMap<usize, (WeakKey, Value)>,
    swept: usize,
}

impl WeakColl {
    pub fn get(&self, k: &Value) -> Option<Value> {
        let (id, _) = WeakKey::of(k)?;
        let (key, v) = self.map.get(&id)?;
        key.alive().then(|| v.clone())
    }

    pub fn has(&self, k: &Value) -> bool {
        self.get(k).is_some()
    }

    pub fn set(&mut self, k: &Value, v: Value) -> bool {
        let Some((id, key)) = WeakKey::of(k) else { return false };
        if self.map.len() >= (2 * self.swept).max(64) {
            self.sweep();
        }
        self.map.insert(id, (key, v));
        true
    }

    pub fn delete(&mut self, k: &Value) -> bool {
        let Some((id, _)) = WeakKey::of(k) else { return false };
        self.map.remove(&id).is_some_and(|(key, _)| key.alive())
    }

    /// Tira as entradas cujas chaves já morreram.
    pub fn sweep(&mut self) {
        self.map.retain(|_, (k, _)| k.alive());
        self.swept = self.map.len();
    }
}

#[derive(Default)]
pub struct Collection {
    pub entries: Vec<Option<(Value, Value)>>,
    pub index: crate::fxhash::FxHashMap<MapKey, usize>,
}

impl Collection {
    pub fn get(&self, k: &Value) -> Option<Value> {
        let i = *self.index.get(&MapKey::of(k))?;
        self.entries[i].as_ref().map(|e| e.1.clone())
    }

    pub fn has(&self, k: &Value) -> bool {
        self.index.contains_key(&MapKey::of(k))
    }

    pub fn set(&mut self, k: Value, v: Value) {
        let k = match k {
            Value::Number(n) if n == 0.0 => Value::Number(0.0),
            other => other,
        };
        let key = MapKey::of(&k);
        match self.index.get(&key) {
            Some(&i) => self.entries[i] = Some((k, v)),
            None => {
                self.index.insert(key, self.entries.len());
                self.entries.push(Some((k, v)));
            }
        }
    }

    pub fn delete(&mut self, k: &Value) -> bool {
        match self.index.remove(&MapKey::of(k)) {
            Some(i) => {
                self.entries[i] = None;
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        for e in self.entries.iter_mut() {
            *e = None;
        }
        self.index.clear();
    }

    pub fn size(&self) -> usize {
        self.index.len()
    }
}

/// Tipo de elemento de um typed array.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaKind {
    I8,
    U8,
    U8C,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
    BI64,
    BU64,
}

impl TaKind {
    pub const ALL: [TaKind; 11] = [
        TaKind::I8,
        TaKind::U8,
        TaKind::U8C,
        TaKind::I16,
        TaKind::U16,
        TaKind::I32,
        TaKind::U32,
        TaKind::F32,
        TaKind::F64,
        TaKind::BI64,
        TaKind::BU64,
    ];

    pub fn size(self) -> usize {
        match self {
            TaKind::I8 | TaKind::U8 | TaKind::U8C => 1,
            TaKind::I16 | TaKind::U16 => 2,
            TaKind::I32 | TaKind::U32 | TaKind::F32 => 4,
            TaKind::F64 | TaKind::BI64 | TaKind::BU64 => 8,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TaKind::I8 => "Int8Array",
            TaKind::U8 => "Uint8Array",
            TaKind::U8C => "Uint8ClampedArray",
            TaKind::I16 => "Int16Array",
            TaKind::U16 => "Uint16Array",
            TaKind::I32 => "Int32Array",
            TaKind::U32 => "Uint32Array",
            TaKind::F32 => "Float32Array",
            TaKind::F64 => "Float64Array",
            TaKind::BI64 => "BigInt64Array",
            TaKind::BU64 => "BigUint64Array",
        }
    }

    /// Posição em `ALL` (e em `Realm::typed_protos`).
    pub fn index(self) -> usize {
        TaKind::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    pub fn is_bigint(self) -> bool {
        matches!(self, TaKind::BI64 | TaKind::BU64)
    }

    /// Lê um elemento de `b` (little-endian, `b.len() >= size()`).
    pub fn decode(self, b: &[u8]) -> Value {
        let mut arr = [0u8; 8];
        arr[..self.size()].copy_from_slice(&b[..self.size()]);
        match self {
            TaKind::I8 => Value::Number(arr[0] as i8 as f64),
            TaKind::U8 | TaKind::U8C => Value::Number(arr[0] as f64),
            TaKind::I16 => Value::Number(i16::from_le_bytes([arr[0], arr[1]]) as f64),
            TaKind::U16 => Value::Number(u16::from_le_bytes([arr[0], arr[1]]) as f64),
            TaKind::I32 => Value::Number(i32::from_le_bytes([arr[0], arr[1], arr[2], arr[3]]) as f64),
            TaKind::U32 => Value::Number(u32::from_le_bytes([arr[0], arr[1], arr[2], arr[3]]) as f64),
            TaKind::F32 => Value::Number(f32::from_le_bytes([arr[0], arr[1], arr[2], arr[3]]) as f64),
            TaKind::F64 => Value::Number(f64::from_le_bytes(arr)),
            TaKind::BI64 => Value::BigInt(Rc::new(crate::bigint::BigInt::from_i64(i64::from_le_bytes(arr)))),
            TaKind::BU64 => Value::BigInt(Rc::new(crate::bigint::BigInt::from_u64(u64::from_le_bytes(arr)))),
        }
    }

    /// Codifica um número (tipos não-BigInt) em little-endian; devolve os bytes usados.
    pub fn encode_number(self, n: f64, out: &mut [u8; 8]) -> usize {
        use crate::conv::to_int32;
        match self {
            TaKind::I8 | TaKind::U8 => out[0] = to_int32(n) as u8,
            TaKind::U8C => {
                // ToUint8Clamp: arredonda para o mais próximo, empate para o par
                let c = if n.is_nan() { 0.0 } else { n.clamp(0.0, 255.0) };
                out[0] = c.round_ties_even() as u8;
            }
            TaKind::I16 | TaKind::U16 => out[..2].copy_from_slice(&(to_int32(n) as u16).to_le_bytes()),
            TaKind::I32 | TaKind::U32 => out[..4].copy_from_slice(&(to_int32(n) as u32).to_le_bytes()),
            TaKind::F32 => out[..4].copy_from_slice(&(n as f32).to_le_bytes()),
            TaKind::F64 => out.copy_from_slice(&n.to_le_bytes()),
            TaKind::BI64 | TaKind::BU64 => return 0,
        }
        self.size()
    }
}

/// Bytes de um ArrayBuffer (compartilhados com as vistas). Desanexado, o
/// vetor fica vazio: as vistas antigas passam a ver tamanho 0.
pub type Bytes = Rc<RefCell<Vec<u8>>>;

/// Um ArrayBuffer.
pub struct BufferData {
    pub bytes: Bytes,
    /// Tamanho máximo (Some = redimensionável).
    pub max: Option<usize>,
    pub detached: bool,
}

impl BufferData {
    pub fn new(bytes: Vec<u8>) -> BufferData {
        BufferData { bytes: Rc::new(RefCell::new(bytes)), max: None, detached: false }
    }
}

/// Uma vista sobre um ArrayBuffer (typed array ou DataView).
pub struct TypedArrayData {
    pub data: Bytes,
    pub buffer: ObjRef,
    pub kind: TaKind,
    /// Em bytes.
    pub offset: usize,
    /// Em elementos (DataView: em bytes). Ignorado se `tracking`.
    pub length: usize,
    /// Acompanha o tamanho do buffer (redimensionável, criada sem tamanho).
    pub tracking: bool,
}

impl TypedArrayData {
    /// Tamanho em elementos para um buffer de `blen` bytes (None = fora dos limites).
    #[inline]
    pub fn len_in(&self, blen: usize) -> Option<usize> {
        if self.tracking {
            if self.offset > blen { None } else { Some((blen - self.offset) / self.kind.size()) }
        } else if self.offset + self.length * self.kind.size() > blen {
            None
        } else {
            Some(self.length)
        }
    }

    /// Tamanho atual em elementos (0 fora dos limites ou desanexado).
    pub fn len(&self) -> usize {
        self.len_in(self.data.borrow().len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Tamanho atual em bytes (0 fora dos limites ou desanexado).
    pub fn byte_len(&self) -> usize {
        self.len() * self.kind.size()
    }

    /// Lê o elemento `i` (None fora dos limites).
    #[inline]
    pub fn get(&self, i: usize) -> Option<Value> {
        let d = self.data.borrow();
        if i >= self.len_in(d.len())? {
            return None;
        }
        let start = self.offset + i * self.kind.size();
        Some(self.kind.decode(&d[start..]))
    }

    /// Grava um número (já convertido) no elemento `i`.
    pub fn set_number(&self, i: usize, n: f64) {
        let mut b = [0u8; 8];
        let sz = self.kind.encode_number(n, &mut b);
        if sz > 0 {
            self.write(i, &b[..sz]);
        }
    }

    pub fn set_bigint(&self, i: usize, b: &crate::bigint::BigInt) {
        if self.kind.is_bigint() {
            self.write(i, &b.to_u64_wrapping().to_le_bytes());
        }
    }

    fn write(&self, i: usize, bytes: &[u8]) {
        let mut d = self.data.borrow_mut();
        if self.len_in(d.len()).is_some_and(|n| i < n) {
            let start = self.offset + i * self.kind.size();
            d[start..start + bytes.len()].copy_from_slice(bytes);
        }
    }
}

pub enum Kind {
    Ordinary,
    Array(Vec<Value>),
    Function(Func),
    Error,
    Boolean(bool),
    Number(f64),
    String(JsStr),
    Symbol(Sym),
    Map(Collection),
    Set(Collection),
    /// WeakMap / WeakSet: as chaves não ficam presas.
    WeakMap(WeakColl),
    WeakSet(WeakColl),
    Date(f64),
    Arguments,
    ArrayIter { target: Value, index: usize, kind: IterKind },
    MapIter { target: ObjRef, index: usize, kind: IterKind },
    StringIter { s: JsStr, pos: usize },
    ForIn { keys: Vec<JsStr>, index: usize, target: Option<ObjRef> },
    /// Registro de iterador em uso (for-of, desestruturação, spread).
    IterRecord { iter: Value, next: Value, done: bool, sync_wrapped: bool },
    RegExp { source: JsStr, flags: JsStr, prog: Rc<crate::regex::Program> },
    /// Iterador de String.prototype.matchAll.
    RegExpStringIter { rx: ObjRef, s: JsStr, global: bool, unicode: bool, done: bool },
    /// Alvo fraco (símbolos ficam fortes: não são coletados).
    WeakRef(WeakKey),
    Generator(Box<crate::builtins::generator::GenData>),
    Promise(Box<crate::builtins::promise::PromiseData>),
    /// Dados das funções resolve/reject de uma promessa.
    PromiseResolver { promise: ObjRef, done: Rc<std::cell::Cell<bool>> },
    /// Função async pausada num await.
    Async(Box<crate::builtins::promise::AsyncData>),
    /// Objeto embrulhando um BigInt (`Object(1n)`).
    BigIntBox(Rc<crate::bigint::BigInt>),
    ArrayBuffer(BufferData),
    /// Objeto do hospedeiro (o navegador): `class` e `id` têm o significado
    /// que ele der (ex.: nó do DOM com o NodeId).
    Host { class: u32, id: usize },
    /// Objetos do Intl (NumberFormat, DateTimeFormat...).
    Intl(Box<crate::builtins::intl::IntlObj>),
    /// Proxy (alvo e handler; None depois de revogado).
    Proxy { target: Option<ObjRef>, handler: Option<ObjRef> },
    TypedArray(Box<TypedArrayData>),
    DataView(Box<TypedArrayData>),
    AsyncGenerator(Box<crate::builtins::async_generator::AsyncGenData>),
}

pub struct Object {
    pub proto: Option<ObjRef>,
    pub props: PropMap,
    pub kind: Kind,
    pub extensible: bool,
    /// Congelado (para os elementos de arrays, que não têm atributos próprios).
    pub frozen: bool,
    /// Nome para Object.prototype.toString ("[object X]").
    pub tag: &'static str,
    /// Uso do coletor de ciclos (época e posição na coleta atual).
    pub gc: std::cell::Cell<u64>,
}

impl Object {
    pub fn new(proto: Option<ObjRef>, kind: Kind) -> Object {
        let tag = match &kind {
            Kind::Array(_) => "Array",
            Kind::Function(_) => "Function",
            Kind::Error => "Error",
            Kind::Boolean(_) => "Boolean",
            Kind::Number(_) => "Number",
            Kind::String(_) => "String",
            Kind::Symbol(_) => "Symbol",
            Kind::Map(_) => "Map",
            Kind::Set(_) => "Set",
            Kind::Date(_) => "Date",
            Kind::Arguments => "Arguments",
            Kind::RegExp { .. } => "RegExp",
            Kind::Promise(_) => "Promise",
            Kind::Generator(_) => "Generator",
            _ => "Object",
        };
        Object {
            proto,
            props: PropMap::default(),
            kind,
            extensible: true,
            frozen: false,
            tag,
            gc: std::cell::Cell::new(0),
        }
    }

    pub fn own(&self, k: &PropKey) -> Option<Property> {
        if let Kind::TypedArray(t) = &self.kind
            && let Some(n) = k.numeric_index()
        {
            return integer_index(n).and_then(|i| t.get(i)).map(Property::data);
        }
        if let Kind::Array(v) = &self.kind {
            if let Some(i) = k.as_index() {
                match v.get(i as usize) {
                    Some(Value::Empty) | None => {}
                    Some(x) => {
                        return Some(Property {
                            slot: Slot::Data {
                                value: x.clone(),
                                writable: !self.frozen,
                            },
                            enumerable: true,
                            configurable: !self.frozen,
                        });
                    }
                }
            }
            if let PropKey::Str(s) = k
                && s.eq_str("length")
            {
                return Some(Property {
                    slot: Slot::Data {
                        value: Value::Number(v.len() as f64),
                        writable: !self.frozen,
                    },
                    enumerable: false,
                    configurable: false,
                });
            }
        }
        if let Kind::String(s) = &self.kind {
            if let Some(i) = k.as_index()
                && (i as usize) < s.len()
            {
                return Some(Property {
                    slot: Slot::Data {
                        value: Value::String(s.slice(i as usize, i as usize + 1)),
                        writable: false,
                    },
                    enumerable: true,
                    configurable: false,
                });
            }
            if let PropKey::Str(n) = k
                && n.eq_str("length")
            {
                return Some(Property {
                    slot: Slot::Data {
                        value: Value::Number(s.len() as f64),
                        writable: false,
                    },
                    enumerable: false,
                    configurable: false,
                });
            }
        }
        self.props.get(k).cloned()
    }

    /// Chaves próprias na ordem do spec (inclui índices de arrays/strings).
    pub fn own_keys(&self) -> Vec<PropKey> {
        let mut keys = Vec::new();
        match &self.kind {
            Kind::TypedArray(t) => {
                for i in 0..t.len() {
                    keys.push(PropKey::Str(JsStr::new(&i.to_string())));
                }
            }
            Kind::Array(v) => {
                for (i, x) in v.iter().enumerate() {
                    if !matches!(x, Value::Empty) {
                        keys.push(PropKey::Str(JsStr::new(&i.to_string())));
                    }
                }
            }
            Kind::String(s) => {
                for i in 0..s.len() {
                    keys.push(PropKey::Str(JsStr::new(&i.to_string())));
                }
                keys.extend(self.props.ordered_keys());
                keys.push(PropKey::str("length"));
                return keys;
            }
            _ => {}
        }
        let props = self.props.ordered_keys();
        if matches!(self.kind, Kind::Array(_)) {
            // índices esparsos guardados fora do vetor
            let mut extra: Vec<(u32, PropKey)> = Vec::new();
            for k in props.iter() {
                if let Some(i) = k.as_index() {
                    extra.push((i, k.clone()));
                }
            }
            if !extra.is_empty() {
                keys.extend(extra.into_iter().map(|(_, k)| k));
                keys.sort_by_key(|k| k.as_index().unwrap_or(u32::MAX));
                keys.extend(props.into_iter().filter(|k| k.as_index().is_none()));
                keys.push(PropKey::str("length"));
                return keys;
            }
        }
        keys.extend(props);
        if matches!(self.kind, Kind::Array(_)) {
            keys.push(PropKey::str("length"));
        }
        keys
    }

    /// Typed array com chave numérica: a busca não sobe para o protótipo.
    #[inline]
    pub fn ta_numeric(&self, k: &PropKey) -> Option<f64> {
        match &self.kind {
            Kind::TypedArray(_) => k.numeric_index(),
            _ => None,
        }
    }

    pub fn is_enumerable(&self, k: &PropKey) -> bool {
        self.own(k).is_some_and(|p| p.enumerable)
    }
}

/// Ambiente em tempo de execução: as variáveis de um escopo.
pub struct Scope {
    pub slots: Vec<Value>,
    pub parent: Option<Env>,
    /// Uso do coletor de ciclos.
    pub gc: std::cell::Cell<u64>,
}

pub type Env = Rc<RefCell<Scope>>;

pub fn new_env(size: usize, parent: Option<Env>) -> Env {
    env_with(vec![Value::Empty; size], parent)
}

/// Ambiente novo com essas variáveis (registrado no coletor de ciclos).
pub fn env_with(slots: Vec<Value>, parent: Option<Env>) -> Env {
    let e = Rc::new(RefCell::new(Scope { slots, parent, gc: std::cell::Cell::new(0) }));
    crate::gc::track_env(&e);
    e
}
