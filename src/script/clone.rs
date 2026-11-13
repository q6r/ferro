//! Clone estruturado (HTML: StructuredSerializeWithTransfer e
//! StructuredDeserializeWithTransfer).
//!
//! Um valor JS vira uma árvore [`Cloned`] feita só de dados (pode ir para
//! outra thread: postMessage dos workers, MessagePort, BroadcastChannel) e
//! depois volta a ser valor em outra realm. `structuredClone` faz as duas
//! metades na mesma realm. Os erros e as mensagens seguem o Chrome.
//!
//! Objetos da plataforma feitos no prelúdio (Blob, File, CryptoKey,
//! ImageData) passam por dois ganchos JS: um diz o que guardar
//! (`[etiqueta, dados]`) e o outro remonta o objeto do outro lado.

use std::collections::HashMap;

use ferro_js::bigint::BigInt;
use ferro_js::value::{BufferData, Kind, ObjRef, Object, PropKey, Property, TaKind, TypedArrayData};
use ferro_js::{JsStr, Value, Vm};

/// `Kind::Host { class }` das MessagePort (o `id` é o da porta no registro).
pub(crate) const PORT: u32 = 2;

/// Texto UTF-16 (strings JS podem ter metades de surrogate soltas).
type Units = Box<[u16]>;

/// Valor serializado: só dados, sem nada preso à realm de origem.
#[derive(Clone, Debug)]
pub(crate) enum Cloned {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    /// Em decimal.
    BigInt(String),
    String(Units),
    /// Objeto que já apareceu antes (ciclos e repetições): posição na ordem
    /// em que os objetos foram visitados.
    Ref(u32),
    /// Propriedades próprias enumeráveis, na ordem.
    Object(Vec<(Units, Cloned)>),
    /// `items` com [`Cloned::Hole`] nos buracos; `extra`: outras propriedades.
    Array { len: u32, items: Vec<Cloned>, extra: Vec<(Units, Cloned)> },
    Hole,
    Date(f64),
    RegExp { source: Units, flags: Units },
    Map(Vec<(Cloned, Cloned)>),
    Set(Vec<Cloned>),
    /// `name` é um dos 7 tipos nativos (os outros viram "Error").
    Error { name: &'static str, message: Option<Units>, stack: Option<Units>, cause: Option<Box<Cloned>> },
    BoolObj(bool),
    NumberObj(f64),
    StringObj(Units),
    BigIntObj(String),
    /// ArrayBuffer copiado (`max`: redimensionável).
    Buffer { bytes: Vec<u8>, max: Option<usize> },
    /// ArrayBuffer transferido: posição em [`Message::buffers`].
    Transferred(u32),
    /// Typed array (`kind`) ou DataView (`None`) sobre um buffer; `length`
    /// None = acompanha o tamanho do buffer.
    View { kind: Option<TaKind>, buffer: Box<Cloned>, offset: usize, length: Option<usize> },
    /// Objeto da plataforma feito no prelúdio.
    Host { tag: String, data: Box<Cloned> },
    /// MessagePort transferida: posição em [`Message::ports`].
    Port(u32),
}

/// Uma mensagem pronta para atravessar threads.
#[derive(Clone, Debug)]
pub(crate) struct Message {
    pub value: Cloned,
    /// ArrayBuffers transferidos: (bytes, tamanho máximo).
    pub buffers: Vec<(Vec<u8>, Option<usize>)>,
    /// MessagePorts transferidas (ids no registro de portas).
    pub ports: Vec<u64>,
}

impl Cloned {
    /// Resumo curto (depuração: `FERRO_WORKER_LOG`).
    pub fn summary(&self) -> String {
        let text = |u: &[u16]| String::from_utf16_lossy(&u[..u.len().min(40)]);
        match self {
            Cloned::String(u) => format!("{:?}", text(u)),
            Cloned::Object(p) => {
                let keys: Vec<String> = p.iter().take(6).map(|(k, v)| format!("{}: {}", text(k), v.summary())).collect();
                format!("{{{}}}", keys.join(", "))
            }
            Cloned::Array { len, .. } => format!("[{len}]"),
            Cloned::Number(n) => n.to_string(),
            Cloned::Bool(b) => b.to_string(),
            Cloned::Null => "null".into(),
            Cloned::Undefined => "undefined".into(),
            other => format!("{:.40}", format!("{other:?}")),
        }
    }
}

/// O que veio na lista de transferência.
pub(crate) struct Transfer {
    pub buffers: Vec<ObjRef>,
    /// (objeto, id da porta).
    pub ports: Vec<(ObjRef, u64)>,
}

/// DOMException "DataCloneError" com o contexto (`Failed to execute ...: `).
pub(crate) fn clone_error(vm: &mut Vm, ctx: &str, msg: &str) -> Value {
    super::dom::dom_error(vm, "DataCloneError", &format!("{ctx}{msg}"))
}

/// Confere a lista de transferência (sem mexer em nada ainda).
pub(crate) fn transfer_list(vm: &mut Vm, list: &[Value], ctx: &str) -> Result<Transfer, Value> {
    let mut t = Transfer { buffers: Vec::new(), ports: Vec::new() };
    for (i, v) in list.iter().enumerate() {
        let Value::Object(o) = v else {
            return Err(clone_error(vm, ctx, &format!("Value at index {i} does not have a transferable type.")));
        };
        let kind = match &o.borrow().kind {
            Kind::ArrayBuffer(b) => Some((false, b.detached)),
            Kind::Host { class: PORT, .. } => Some((true, false)),
            _ => None,
        };
        match kind {
            Some((false, detached)) => {
                if t.buffers.iter().any(|b| b.ptr_eq(o)) {
                    return Err(clone_error(vm, ctx, &format!("ArrayBuffer at index {i} is a duplicate of an earlier ArrayBuffer.")));
                }
                if detached {
                    return Err(clone_error(vm, ctx, &format!("ArrayBuffer at index {i} is already detached.")));
                }
                t.buffers.push(o.clone());
            }
            Some((true, _)) => {
                if t.ports.iter().any(|(p, _)| p.ptr_eq(o)) {
                    return Err(clone_error(vm, ctx, &format!("Message port at index {i} is a duplicate of an earlier port.")));
                }
                let id = match o.borrow().kind {
                    Kind::Host { id, .. } => id as u64,
                    _ => 0,
                };
                t.ports.push((o.clone(), id));
            }
            None => return Err(clone_error(vm, ctx, &format!("Value at index {i} does not have a transferable type."))),
        }
    }
    Ok(t)
}

struct Ser<'a> {
    /// Objeto visitado → posição.
    memory: HashMap<usize, u32>,
    count: u32,
    /// Objetos da lista de transferência → `Transferred(i)` / `Port(i)`.
    transferred: HashMap<usize, Cloned>,
    plat_out: &'a Value,
    ctx: &'a str,
    depth: u32,
}

/// Serializa `value`. Os buffers transferidos só são desanexados se tudo der
/// certo; as portas ficam para quem chamou (`t.ports`, na mesma ordem de
/// [`Message::ports`]).
pub(crate) fn serialize(vm: &mut Vm, value: &Value, t: &Transfer, plat_out: &Value, ctx: &str) -> Result<Message, Value> {
    let mut s = Ser { memory: HashMap::new(), count: 0, transferred: HashMap::new(), plat_out, ctx, depth: 0 };
    for (i, b) in t.buffers.iter().enumerate() {
        s.transferred.insert(b.id(), Cloned::Transferred(i as u32));
    }
    for (i, (p, _)) in t.ports.iter().enumerate() {
        s.transferred.insert(p.id(), Cloned::Port(i as u32));
    }
    let value = s.value(vm, value)?;
    // deu certo: os buffers transferidos mudam de dono
    let mut buffers = Vec::with_capacity(t.buffers.len());
    for b in &t.buffers {
        let mut bm = b.borrow_mut();
        if let Kind::ArrayBuffer(d) = &mut bm.kind {
            let bytes = std::mem::take(&mut *d.bytes.borrow_mut());
            buffers.push((bytes, d.max));
            d.detached = true;
        }
    }
    Ok(Message { value, buffers, ports: t.ports.iter().map(|(_, id)| *id).collect() })
}

/// O que fazer com um objeto, olhando o tipo interno.
enum Plan {
    Array(Vec<Value>),
    Plain,
    Error,
    Date(f64),
    RegExp(JsStr, JsStr),
    Map(Vec<(Value, Value)>),
    Set(Vec<Value>),
    Bool(bool),
    Number(f64),
    Str(JsStr),
    BigInt(String),
    Buffer(Vec<u8>, Option<usize>, bool),
    View(Option<TaKind>, ObjRef, usize, Option<usize>),
    Node(&'static str),
    Port,
    Function,
    Other,
}

impl Ser<'_> {
    fn error(&self, vm: &mut Vm, what: &str) -> Value {
        clone_error(vm, self.ctx, &format!("{what} could not be cloned."))
    }

    fn value(&mut self, vm: &mut Vm, v: &Value) -> Result<Cloned, Value> {
        Ok(match v {
            Value::Undefined | Value::Empty => Cloned::Undefined,
            Value::Null => Cloned::Null,
            Value::Bool(b) => Cloned::Bool(*b),
            Value::Number(n) => Cloned::Number(*n),
            Value::String(s) => Cloned::String(s.units().into()),
            Value::BigInt(b) => Cloned::BigInt(b.to_string_radix(10)),
            Value::Symbol(_) => {
                let d = vm.describe(v);
                return Err(self.error(vm, &d));
            }
            Value::Object(o) => self.object(vm, o)?,
        })
    }

    fn object(&mut self, vm: &mut Vm, o: &ObjRef) -> Result<Cloned, Value> {
        if let Some(t) = self.transferred.get(&o.id()) {
            return Ok(t.clone());
        }
        if let Some(&n) = self.memory.get(&o.id()) {
            return Ok(Cloned::Ref(n));
        }
        if self.depth > 5000 {
            return Err(vm.range_error("Maximum call stack size exceeded"));
        }
        let plan = {
            let b = o.borrow();
            match &b.kind {
                Kind::Array(items) => Plan::Array(items.clone()),
                Kind::Ordinary | Kind::Arguments => Plan::Plain,
                Kind::Error => Plan::Error,
                Kind::Date(t) => Plan::Date(*t),
                Kind::RegExp { source, flags, .. } => Plan::RegExp(source.clone(), flags.clone()),
                Kind::Map(c) => Plan::Map(c.entries.iter().flatten().cloned().collect()),
                Kind::Set(c) => Plan::Set(c.entries.iter().flatten().map(|(k, _)| k.clone()).collect()),
                Kind::Boolean(x) => Plan::Bool(*x),
                Kind::Number(x) => Plan::Number(*x),
                Kind::String(x) => Plan::Str(x.clone()),
                Kind::BigIntBox(x) => Plan::BigInt(x.to_string_radix(10)),
                Kind::ArrayBuffer(d) => Plan::Buffer(if d.detached { Vec::new() } else { d.bytes.borrow().clone() }, d.max, d.detached),
                Kind::TypedArray(t) => Plan::View(Some(t.kind), t.buffer.clone(), t.offset, (!t.tracking).then_some(t.length)),
                Kind::DataView(t) => Plan::View(None, t.buffer.clone(), t.offset, (!t.tracking).then_some(t.length)),
                Kind::Host { class: super::NODE, .. } => Plan::Node(b.tag),
                Kind::Host { class: PORT, .. } => Plan::Port,
                Kind::Function(_) => Plan::Function,
                Kind::Proxy { target: Some(t), .. } if t.is_callable() => Plan::Function,
                _ => Plan::Other,
            }
        };
        match plan {
            Plan::Function => {
                let v = Value::Object(o.clone());
                let text = vm.to_rust_string(&v).unwrap_or_else(|_| "function".into());
                return Err(self.error(vm, &text));
            }
            Plan::Node(tag) => return Err(self.error(vm, &format!("{tag} object"))),
            Plan::Port => {
                return Err(clone_error(vm, self.ctx, "A MessagePort could not be cloned because it was not transferred."));
            }
            Plan::Other => {
                let d = vm.describe(&Value::Object(o.clone()));
                return Err(self.error(vm, &d));
            }
            Plan::Buffer(_, _, true) => {
                return Err(clone_error(vm, self.ctx, "An ArrayBuffer is detached and could not be cloned."));
            }
            _ => {}
        }
        // objeto da plataforma feito no prelúdio? (só os de protótipo próprio)
        let mut host = None;
        if matches!(plan, Plan::Plain) && self.plat_out.is_callable() {
            let proto = o.borrow().proto.clone();
            if proto.is_some_and(|p| !p.ptr_eq(&vm.realm.object_proto)) {
                let r = vm.call(self.plat_out, Value::Undefined, vec![Value::Object(o.clone())])?;
                if let Value::Object(pair) = &r {
                    let tag = vm.get_index(&r, &Value::Number(0.0))?;
                    let tag = vm.to_rust_string(&tag)?;
                    let data = vm.get_index(&Value::Object(pair.clone()), &Value::Number(1.0))?;
                    // "!": objeto da plataforma que não se clona ("URL object")
                    if tag == "!" {
                        let what = vm.to_rust_string(&data)?;
                        return Err(self.error(vm, &what));
                    }
                    host = Some((tag, data));
                }
            }
        }
        let id = self.count;
        self.memory.insert(o.id(), id);
        self.count += 1;
        self.depth += 1;
        let out = self.fill(vm, o, plan, host);
        self.depth -= 1;
        out
    }

    fn fill(&mut self, vm: &mut Vm, o: &ObjRef, plan: Plan, host: Option<(String, Value)>) -> Result<Cloned, Value> {
        if let Some((tag, data)) = host {
            let data = self.value(vm, &data)?;
            return Ok(Cloned::Host { tag, data: Box::new(data) });
        }
        Ok(match plan {
            Plan::Array(items) => {
                let len = items.len() as u32;
                let mut out = Vec::with_capacity(items.len());
                for it in &items {
                    out.push(if matches!(it, Value::Empty) { Cloned::Hole } else { self.value(vm, it)? });
                }
                // propriedades fora do vetor (índices esparsos, nomes)
                let keys = o.borrow().props.ordered_keys();
                let mut extra = Vec::new();
                let mut len = len;
                for k in keys {
                    let PropKey::Str(name) = &k else { continue };
                    if name.eq_str("length") {
                        if let Ok(Value::Number(n)) = vm.get_obj(o, &k) {
                            len = len.max(n as u32);
                        }
                        continue;
                    }
                    if !vm.is_enumerable_own(o, &k)? {
                        continue;
                    }
                    let v = vm.get_obj(o, &k)?;
                    extra.push((name.units().into(), self.value(vm, &v)?));
                }
                Cloned::Array { len, items: out, extra }
            }
            Plan::Plain => Cloned::Object(self.props(vm, o)?),
            Plan::Error => {
                let name = vm.get_str(&Value::Object(o.clone()), "name")?;
                let name = match vm.to_rust_string(&name).unwrap_or_default().as_str() {
                    "EvalError" => "EvalError",
                    "RangeError" => "RangeError",
                    "ReferenceError" => "ReferenceError",
                    "SyntaxError" => "SyntaxError",
                    "TypeError" => "TypeError",
                    "URIError" => "URIError",
                    _ => "Error",
                };
                let own = |o: &ObjRef, k: &str| {
                    o.borrow().props.get(&PropKey::str(k)).and_then(|p| match &p.slot {
                        ferro_js::value::Slot::Data { value, .. } => Some(value.clone()),
                        _ => None,
                    })
                };
                let message = match own(o, "message") {
                    Some(m) => Some(vm.to_string(&m)?.units().into()),
                    None => None,
                };
                let stack = match own(o, "stack") {
                    Some(Value::String(s)) => Some(s.units().into()),
                    _ => None,
                };
                let cause = match own(o, "cause") {
                    Some(c) => Some(Box::new(self.value(vm, &c)?)),
                    None => None,
                };
                Cloned::Error { name, message, stack, cause }
            }
            Plan::Date(t) => Cloned::Date(t),
            Plan::RegExp(s, f) => Cloned::RegExp { source: s.units().into(), flags: f.units().into() },
            Plan::Map(entries) => {
                let mut out = Vec::with_capacity(entries.len());
                for (k, v) in &entries {
                    let k = self.value(vm, k)?;
                    out.push((k, self.value(vm, v)?));
                }
                Cloned::Map(out)
            }
            Plan::Set(items) => {
                let mut out = Vec::with_capacity(items.len());
                for k in &items {
                    out.push(self.value(vm, k)?);
                }
                Cloned::Set(out)
            }
            Plan::Bool(b) => Cloned::BoolObj(b),
            Plan::Number(n) => Cloned::NumberObj(n),
            Plan::Str(s) => Cloned::StringObj(s.units().into()),
            Plan::BigInt(s) => Cloned::BigIntObj(s),
            Plan::Buffer(bytes, max, _) => Cloned::Buffer { bytes, max },
            Plan::View(kind, buffer, offset, length) => {
                let detached = matches!(&buffer.borrow().kind, Kind::ArrayBuffer(d) if d.detached);
                if detached {
                    return Err(clone_error(vm, self.ctx, "An ArrayBuffer is detached and could not be cloned."));
                }
                let b = self.object(vm, &buffer)?;
                Cloned::View { kind, buffer: Box::new(b), offset, length }
            }
            Plan::Node(_) | Plan::Port | Plan::Function | Plan::Other => Cloned::Undefined,
        })
    }

    /// Propriedades próprias enumeráveis com nome de texto, na ordem.
    fn props(&mut self, vm: &mut Vm, o: &ObjRef) -> Result<Vec<(Units, Cloned)>, Value> {
        let mut out = Vec::new();
        for k in vm.own_keys(o)? {
            let PropKey::Str(name) = &k else { continue };
            if !vm.is_enumerable_own(o, &k)? {
                continue;
            }
            let v = vm.get_obj(o, &k)?;
            out.push((name.units().into(), self.value(vm, &v)?));
        }
        Ok(out)
    }
}

/// Remonta a mensagem nesta realm. `ports`: os objetos MessagePort já
/// criados para [`Message::ports`], na mesma ordem.
pub(crate) fn deserialize(vm: &mut Vm, mut msg: Message, ports: &[Value], plat_in: &Value) -> Result<Value, Value> {
    let buffers = msg
        .buffers
        .into_iter()
        .map(|(bytes, max)| {
            let mut d = BufferData::new(bytes);
            d.max = max;
            ObjRef::new(Object::new(Some(vm.realm.array_buffer_proto.clone()), Kind::ArrayBuffer(d)))
        })
        .collect();
    let mut de = De { objs: Vec::new(), buffers, ports, plat_in };
    // os bytes dos buffers copiados passam para os objetos novos (sem outra cópia)
    de.value(vm, &mut msg.value)
}

struct De<'a> {
    objs: Vec<Value>,
    buffers: Vec<ObjRef>,
    ports: &'a [Value],
    plat_in: &'a Value,
}

fn js_str(u: &[u16]) -> JsStr {
    JsStr::from_slice(u)
}

fn bigint(s: &str) -> BigInt {
    match s.strip_prefix('-') {
        Some(d) => BigInt::parse_radix(d, 10).unwrap_or_else(BigInt::zero).neg(),
        None => BigInt::parse_radix(s, 10).unwrap_or_else(BigInt::zero),
    }
}

impl De<'_> {
    /// Guarda o objeto na ordem de visita e devolve o valor.
    fn push(&mut self, o: ObjRef) -> Value {
        let v = Value::Object(o);
        self.objs.push(v.clone());
        v
    }

    fn obj(&self, proto: &ObjRef, kind: Kind) -> ObjRef {
        ObjRef::new(Object::new(Some(proto.clone()), kind))
    }

    fn value(&mut self, vm: &mut Vm, c: &mut Cloned) -> Result<Value, Value> {
        let r = &vm.realm;
        Ok(match c {
            Cloned::Undefined | Cloned::Hole => Value::Undefined,
            Cloned::Null => Value::Null,
            Cloned::Bool(b) => Value::Bool(*b),
            Cloned::Number(n) => Value::Number(*n),
            Cloned::BigInt(s) => Value::BigInt(std::rc::Rc::new(bigint(s))),
            Cloned::String(u) => Value::String(js_str(u)),
            Cloned::Ref(n) => self.objs.get(*n as usize).cloned().unwrap_or(Value::Undefined),
            Cloned::Transferred(i) => self.buffers.get(*i as usize).map_or(Value::Undefined, |b| Value::Object(b.clone())),
            Cloned::Port(i) => self.ports.get(*i as usize).cloned().unwrap_or(Value::Undefined),
            Cloned::Object(props) => {
                let o = vm.new_object();
                let v = self.push(o.clone());
                for (k, x) in props {
                    let x = self.value(vm, x)?;
                    vm.define_data(&o, PropKey::Str(js_str(k)), x);
                }
                v
            }
            Cloned::Array { len, items, extra } => {
                let a = vm.new_array(Vec::new());
                let v = self.push(a.clone());
                let mut vals = Vec::with_capacity((*len as usize).max(items.len()));
                for it in items {
                    vals.push(if matches!(it, Cloned::Hole) { Value::Empty } else { self.value(vm, it)? });
                }
                if vals.len() < *len as usize {
                    vals.resize(*len as usize, Value::Empty);
                }
                if let Kind::Array(dst) = &mut a.borrow_mut().kind {
                    *dst = vals;
                }
                for (k, x) in extra {
                    let x = self.value(vm, x)?;
                    vm.define_data(&a, PropKey::Str(js_str(k)), x);
                }
                v
            }
            Cloned::Date(t) => {
                let o = self.obj(&r.date_proto, Kind::Date(*t));
                self.push(o)
            }
            Cloned::RegExp { source, flags } => {
                let v = vm.new_regexp(js_str(source), js_str(flags))?;
                self.objs.push(v.clone());
                v
            }
            Cloned::Map(entries) => {
                let o = self.obj(&r.map_proto, Kind::Map(Default::default()));
                let v = self.push(o.clone());
                for (k, x) in entries {
                    let k = self.value(vm, k)?;
                    let x = self.value(vm, x)?;
                    if let Kind::Map(m) = &mut o.borrow_mut().kind {
                        m.set(k, x);
                    }
                }
                v
            }
            Cloned::Set(items) => {
                let o = self.obj(&r.set_proto, Kind::Set(Default::default()));
                let v = self.push(o.clone());
                for k in items {
                    let k = self.value(vm, k)?;
                    if let Kind::Set(m) = &mut o.borrow_mut().kind {
                        m.set(k.clone(), k);
                    }
                }
                v
            }
            Cloned::Error { name, message, stack, cause } => {
                let proto = match *name {
                    "EvalError" => r.eval_error_proto.clone(),
                    "RangeError" => r.range_error_proto.clone(),
                    "ReferenceError" => r.reference_error_proto.clone(),
                    "SyntaxError" => r.syntax_error_proto.clone(),
                    "TypeError" => r.type_error_proto.clone(),
                    "URIError" => r.uri_error_proto.clone(),
                    _ => r.error_proto.clone(),
                };
                let o = self.obj(&proto, Kind::Error);
                let v = self.push(o.clone());
                if let Some(m) = message {
                    o.borrow_mut().props.insert(PropKey::str("message"), Property::hidden(Value::String(js_str(m))));
                }
                if let Some(s) = stack {
                    o.borrow_mut().props.insert(PropKey::str("stack"), Property::hidden(Value::String(js_str(s))));
                }
                if let Some(c) = cause {
                    let c = self.value(vm, c)?;
                    o.borrow_mut().props.insert(PropKey::str("cause"), Property::hidden(c));
                }
                v
            }
            Cloned::BoolObj(b) => {
                let o = self.obj(&r.boolean_proto, Kind::Boolean(*b));
                self.push(o)
            }
            Cloned::NumberObj(n) => {
                let o = self.obj(&r.number_proto, Kind::Number(*n));
                self.push(o)
            }
            Cloned::StringObj(u) => {
                let o = self.obj(&r.string_proto, Kind::String(js_str(u)));
                self.push(o)
            }
            Cloned::BigIntObj(s) => {
                let o = self.obj(&r.bigint_proto, Kind::BigIntBox(std::rc::Rc::new(bigint(s))));
                self.push(o)
            }
            Cloned::Buffer { bytes, max } => {
                let mut d = BufferData::new(std::mem::take(bytes));
                d.max = *max;
                let o = self.obj(&r.array_buffer_proto, Kind::ArrayBuffer(d));
                self.push(o)
            }
            Cloned::View { kind, buffer, offset, length } => {
                // a vista vem antes do buffer na ordem de visita
                let slot = self.objs.len();
                self.objs.push(Value::Undefined);
                let Value::Object(buf) = self.value(vm, buffer)? else { return Ok(Value::Undefined) };
                let data = match &buf.borrow().kind {
                    Kind::ArrayBuffer(d) => d.bytes.clone(),
                    _ => Default::default(),
                };
                let ta = Box::new(TypedArrayData {
                    data,
                    buffer: buf.clone(),
                    kind: kind.unwrap_or(TaKind::U8),
                    offset: *offset,
                    length: length.unwrap_or(0),
                    tracking: length.is_none(),
                    immutable: false,
                });
                let r = &vm.realm;
                let o = match kind {
                    Some(k) => self.obj(&r.typed_protos[k.index()], Kind::TypedArray(ta)),
                    None => self.obj(&r.data_view_proto, Kind::DataView(ta)),
                };
                let v = Value::Object(o);
                self.objs[slot] = v.clone();
                v
            }
            Cloned::Host { tag, data } => {
                let slot = self.objs.len();
                self.objs.push(Value::Undefined);
                let data = self.value(vm, data)?;
                let v = if self.plat_in.is_callable() {
                    vm.call(self.plat_in, Value::Undefined, vec![Value::str(tag), data])?
                } else {
                    Value::Undefined
                };
                self.objs[slot] = v.clone();
                v
            }
        })
    }
}

/// `__native.structuredClone(valor, transferência, contexto, platOut, platIn)`:
/// as duas metades do clone na mesma realm. As portas transferidas mudam de
/// objeto (o antigo fica inútil), como no Chrome.
pub(crate) fn structured_clone(vm: &mut Vm, a: &mut ferro_js::value::CallArgs) -> Result<Value, Value> {
    let ctx = vm.to_rust_string(&a.arg(2))?;
    let list = match a.arg(1) {
        Value::Undefined | Value::Null => Vec::new(),
        v => vm.iterate_to_vec(&v)?,
    };
    let t = transfer_list(vm, &list, &ctx)?;
    let msg = serialize(vm, &a.arg(0), &t, &a.arg(3), &ctx)?;
    let ports = super::messaging::adopt_ports(vm, &t, &msg.ports)?;
    deserialize(vm, msg, &ports, &a.arg(4))
}

#[cfg(test)]
mod testes {
    use super::*;

    fn roda(src: &str) -> String {
        let src = src.to_string();
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || {
                let mut e = ferro_js::Engine::new();
                // ida e volta pela árvore (sem os ganchos do prelúdio)
                fn ida_e_volta(vm: &mut Vm, a: &mut ferro_js::value::CallArgs) -> Result<Value, Value> {
                    let list = match a.arg(1) {
                        Value::Undefined => Vec::new(),
                        v => vm.iterate_to_vec(&v)?,
                    };
                    let t = transfer_list(vm, &list, "ctx: ")?;
                    let msg = serialize(vm, &a.arg(0), &t, &Value::Undefined, "ctx: ")?;
                    // atravessa uma thread de verdade
                    let msg = std::thread::spawn(move || msg).join().unwrap();
                    deserialize(vm, msg, &[], &Value::Undefined)
                }
                let g = e.vm.realm.global.clone();
                e.vm.realm.method(&g, "clona", 2, ida_e_volta);
                // o DOMException de verdade vem do prelúdio
                let _ = e.eval("globalThis.DOMException = class DOMException extends Error { constructor(m, n) { super(m); this.name = n; } }", "dom");
                match e.eval(&src, "teste") {
                    Ok(Value::String(s)) => s.to_rust(),
                    Ok(v) => e.inspect(&v),
                    Err(err) => e.error_text(&err),
                }
            })
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn primitivos_objetos_e_ciclos() {
        assert_eq!(
            roda(r#"const o = { a: 1, b: 'x\uD800y', c: [1, , 3], d: null, e: undefined, f: -0, g: 12n, h: { i: true } };
                    o.self = o; o.c.extra = 'e';
                    const c = clona(o);
                    [c !== o, c.self === c, c.b === 'x\uD800y', 1 in c.c, c.c.length, c.c.extra, Object.is(c.f, -0),
                     c.g === 12n, c.h.i, 'e' in c, JSON.stringify(Object.keys(c))].join()"#),
            r#"true,true,true,false,3,e,true,true,true,true,["a","b","c","d","e","f","g","h","self"]"#
        );
    }

    #[test]
    fn tipos_embutidos() {
        assert_eq!(
            roda(r#"const d = new Date(5), r = /a+/gi, m = new Map([[1, { k: 2 }]]), s = new Set(['z']);
                    const e = new TypeError('ruim', { cause: 7 }); e.extra = 1;
                    const c = clona([d, r, m, s, e, Object(3), Object('t'), Object(false), Object(5n), new RangeError('x')]);
                    [c[0] instanceof Date && c[0].getTime(), c[1].source + c[1].flags, c[2].get(1).k, c[3].has('z'),
                     c[4] instanceof TypeError, c[4].message, c[4].cause, 'extra' in c[4], c[4].stack === e.stack,
                     typeof c[5], c[5] + 1, c[6] + '!', typeof c[7], typeof c[8], c[9].name].join()"#),
            "5,a+gi,2,true,true,ruim,7,false,true,object,4,t!,object,object,RangeError"
        );
    }

    #[test]
    fn buffers_vistas_e_transferencia() {
        assert_eq!(
            roda(r#"const b = new ArrayBuffer(8); const u = new Uint8Array(b, 2, 3); u[0] = 9;
                    const dv = new DataView(b); const f = new Float64Array([1.5]);
                    const c = clona({ b, u, dv, f });
                    const r = [c.u.buffer === c.b, c.dv.buffer === c.b, c.u.byteOffset, c.u.length, c.u[0], c.f[0], b.byteLength];
                    const t = new ArrayBuffer(4); new Uint8Array(t)[1] = 7;
                    const c2 = clona({ t, v: new Uint16Array(t) }, [t]);
                    r.push(t.byteLength, t.detached, c2.t.byteLength, new Uint8Array(c2.t)[1], c2.v.buffer === c2.t);
                    r.join()"#),
            "true,true,2,3,9,1.5,8,0,true,4,7,true"
        );
    }

    #[test]
    fn erros_como_o_chrome() {
        assert_eq!(roda("try { clona({ f() {} }) } catch (e) { e.name + ': ' + e.message }"), "DataCloneError: ctx: f() {} could not be cloned.");
        assert_eq!(roda("try { clona(Symbol('s')) } catch (e) { e.message }"), "ctx: Symbol(s) could not be cloned.");
        assert_eq!(roda("try { clona(new WeakMap()) } catch (e) { e.message }"), "ctx: #<WeakMap> could not be cloned.");
        assert_eq!(roda("try { clona(1, [{}]) } catch (e) { e.name + ': ' + e.message }"), "DataCloneError: ctx: Value at index 0 does not have a transferable type.");
        assert_eq!(
            roda("const b = new ArrayBuffer(1); try { clona(1, [b, b]) } catch (e) { e.message }"),
            "ctx: ArrayBuffer at index 1 is a duplicate of an earlier ArrayBuffer."
        );
        assert_eq!(
            roda("const b = new ArrayBuffer(1); clona(1, [b]); try { clona(b) } catch (e) { e.message }"),
            "ctx: An ArrayBuffer is detached and could not be cloned."
        );
        // falhou no meio: nada foi desanexado
        assert_eq!(roda("const b = new ArrayBuffer(2); try { clona([b, () => 1], [b]) } catch (e) {} b.byteLength"), "2");
    }
}
