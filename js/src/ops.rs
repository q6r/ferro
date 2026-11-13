//! Operações abstratas do spec usadas pela VM e pelos embutidos:
//! acesso a propriedades, conversões, igualdade, iteração e erros.

use std::collections::HashSet;
use std::rc::Rc;

use crate::ast::FnKind;
use crate::compiler::Code;
use crate::conv::{number_to_string, string_to_number, to_uint32};
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Hint, Vm, same_value_zero, strict_equals, truthy};

/// Operações numéricas binárias (Number ou BigInt).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    UShr,
}

pub fn num_op(k: Arith, x: f64, y: f64) -> f64 {
    use crate::conv::{to_int32, to_uint32};
    match k {
        Arith::Add => x + y,
        Arith::Sub => x - y,
        Arith::Mul => x * y,
        Arith::Div => x / y,
        Arith::Mod => crate::vm::js_rem(x, y),
        Arith::Exp => crate::vm::js_pow(x, y),
        Arith::And => (to_int32(x) & to_int32(y)) as f64,
        Arith::Or => (to_int32(x) | to_int32(y)) as f64,
        Arith::Xor => (to_int32(x) ^ to_int32(y)) as f64,
        Arith::Shl => to_int32(x).wrapping_shl(to_uint32(y) & 31) as f64,
        Arith::Shr => (to_int32(x) >> (to_uint32(y) & 31)) as f64,
        Arith::UShr => ((to_int32(x) as u32) >> (to_uint32(y) & 31)) as f64,
    }
}

/// Representação curta de um valor para mensagens de erro.
pub fn key_display(k: &PropKey) -> String {
    match k {
        PropKey::Str(s) => s.to_rust(),
        PropKey::Sym(s) => format!("Symbol({})", s.description().map(|d| d.to_rust()).unwrap_or_default()),
    }
}

pub fn index_key(i: usize) -> PropKey {
    PropKey::Str(JsStr::new(&i.to_string()))
}

impl Vm {
    // ------------------------------------------------------------ erros

    pub fn make_error(&mut self, proto: &ObjRef, msg: &str) -> Value {
        let o = ObjRef::new(Object::new(Some(proto.clone()), Kind::Error));
        if !msg.is_empty() {
            o.borrow_mut().props.insert(PropKey::str("message"), Property::hidden(Value::str(msg)));
        }
        let name = data_in_chain(proto, "name").map(|v| match v {
            Value::String(s) => s.to_rust(),
            _ => "Error".into(),
        });
        let name = name.unwrap_or_else(|| "Error".into());
        let header = if msg.is_empty() { name } else { format!("{name}: {msg}") };
        let stack = self.stack_trace(&header);
        o.borrow_mut().props.insert(PropKey::str("stack"), Property::hidden(Value::str(&stack)));
        Value::Object(o)
    }

    /// Texto de `error.stack`: cabeçalho e as chamadas ativas.
    pub fn stack_trace(&self, header: &str) -> String {
        let mut s = header.to_string();
        let n = self.trace.len();
        for (count, (i, t)) in self.trace.iter().enumerate().rev().enumerate() {
            if count >= 10 {
                break;
            }
            let pc = if i == n - 1 { self.cur_pc } else { t.pc };
            let line = t.code.line_at(pc);
            let fname = t.code.name.to_rust();
            if t.code.is_script || fname.is_empty() {
                s.push_str(&format!("\n    at {}:{}", t.code.file, line));
            } else {
                s.push_str(&format!("\n    at {} ({}:{})", fname, t.code.file, line));
            }
        }
        s
    }

    pub fn type_error(&mut self, msg: impl Into<String>) -> Value {
        let p = self.realm.type_error_proto.clone();
        self.make_error(&p, &msg.into())
    }

    pub fn range_error(&mut self, msg: impl Into<String>) -> Value {
        let p = self.realm.range_error_proto.clone();
        self.make_error(&p, &msg.into())
    }

    pub fn reference_error(&mut self, msg: impl Into<String>) -> Value {
        let p = self.realm.reference_error_proto.clone();
        self.make_error(&p, &msg.into())
    }

    pub fn syntax_error(&mut self, msg: impl Into<String>) -> Value {
        let p = self.realm.syntax_error_proto.clone();
        self.make_error(&p, &msg.into())
    }

    /// Valor resumido para mensagens ("undefined", "#<Object>", ...).
    pub fn describe(&self, v: &Value) -> String {
        match v {
            Value::Undefined | Value::Empty => "undefined".into(),
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => number_to_string(*n),
            Value::String(s) => format!("\"{}\"", s.to_rust()),
            Value::Symbol(s) => format!("Symbol({})", s.description().map(|d| d.to_rust()).unwrap_or_default()),
            Value::BigInt(b) => format!("{}n", b.to_string_radix(10)),
            Value::Object(o) => {
                if o.is_callable() {
                    let name = data_in_chain(o, "name")
                        .and_then(|v| match v {
                            Value::String(s) => Some(s.to_rust()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    if name.is_empty() { "function".into() } else { name }
                } else {
                    let ctor = data_in_chain(o, "constructor")
                        .and_then(|c| match c {
                            Value::Object(c) => data_in_chain(&c, "name"),
                            _ => None,
                        })
                        .and_then(|v| match v {
                            Value::String(s) if !s.is_empty() => Some(s.to_rust()),
                            _ => None,
                        });
                    format!("#<{}>", ctor.unwrap_or_else(|| o.borrow().tag.to_string()))
                }
            }
        }
    }

    // ------------------------------------------------------- criação

    pub fn new_object(&self) -> ObjRef {
        ObjRef::new(Object::new(Some(self.realm.object_proto.clone()), Kind::Ordinary))
    }

    pub fn new_array(&self, v: Vec<Value>) -> ObjRef {
        ObjRef::new(Object::new(Some(self.realm.array_proto.clone()), Kind::Array(v)))
    }

    pub fn array_value(&self, v: Vec<Value>) -> Value {
        Value::Object(self.new_array(v))
    }

    pub fn make_closure(&mut self, code: Rc<Code>, env: Option<Env>, home: Option<ObjRef>) -> ObjRef {
        let is_ctor = code.kind == FnKind::Normal && !code.is_async && !code.is_generator;
        let name = Value::String(code.name.clone());
        let length = code.length;
        let fproto = match (code.is_async, code.is_generator) {
            (true, true) => self.realm.async_generator_function_proto.clone(),
            (true, false) => self.realm.async_function_proto.clone(),
            (false, true) => self.realm.generator_function_proto.clone(),
            _ => self.realm.function_proto.clone(),
        };
        let fo = ObjRef::new(Object::new(
            Some(fproto),
            Kind::Function(Func::Closure(Rc::new(Closure {
                code,
                env,
                this: None,
                new_target: None,
                home,
                fields: None,
                gc: Default::default(),
            }))),
        ));
        {
            let mut b = fo.borrow_mut();
            b.props.insert(PropKey::str("length"), readonly(Value::Number(length as f64)));
            b.props.insert(PropKey::str("name"), readonly(name));
        }
        let is_gen = matches!(&fo.borrow().kind, Kind::Function(Func::Closure(c)) if c.code.is_generator);
        if is_gen {
            let is_async_gen = matches!(&fo.borrow().kind, Kind::Function(Func::Closure(c)) if c.code.is_async);
            let base = if is_async_gen { self.realm.async_generator_proto.clone() } else { self.realm.generator_proto.clone() };
            let proto = ObjRef::new(Object::new(Some(base), Kind::Ordinary));
            fo.borrow_mut().props.insert(
                PropKey::str("prototype"),
                Property {
                    slot: Slot::Data {
                        value: Value::Object(proto),
                        writable: true,
                    },
                    enumerable: false,
                    configurable: false,
                },
            );
        }
        if is_ctor {
            let proto = self.new_object();
            proto.borrow_mut().props.insert(PropKey::str("constructor"), Property::hidden(Value::Object(fo.clone())));
            fo.borrow_mut().props.insert(
                PropKey::str("prototype"),
                Property {
                    slot: Slot::Data {
                        value: Value::Object(proto),
                        writable: true,
                    },
                    enumerable: false,
                    configurable: false,
                },
            );
        }
        fo
    }

    pub fn make_class(&mut self, code: Rc<Code>, env: Option<Env>, sup: Option<Value>) -> Result<(ObjRef, ObjRef), Value> {
        let (proto_parent, ctor_parent) = match sup {
            None => (Some(self.realm.object_proto.clone()), self.realm.function_proto.clone()),
            Some(Value::Null) => (None, self.realm.function_proto.clone()),
            Some(Value::Object(s)) if s.is_constructor() => {
                let pp = self.get_obj(&s, &PropKey::str("prototype"))?;
                let pp = match pp {
                    Value::Object(p) => Some(p),
                    Value::Null => None,
                    _ => return Err(self.type_error("Class extends value does not have valid prototype property")),
                };
                (pp, s)
            }
            Some(v) => {
                let d = self.describe(&v);
                return Err(self.type_error(format!("Class extends value {d} is not a constructor or null")));
            }
        };
        let proto = ObjRef::new(Object::new(proto_parent, Kind::Ordinary));
        let name = Value::String(code.name.clone());
        let length = code.length;
        let fo = ObjRef::new(Object::new(
            Some(ctor_parent),
            Kind::Function(Func::Closure(Rc::new(Closure {
                code,
                env,
                this: None,
                new_target: None,
                home: Some(proto.clone()),
                fields: None,
                gc: Default::default(),
            }))),
        ));
        {
            let mut b = fo.borrow_mut();
            b.props.insert(PropKey::str("length"), readonly(Value::Number(length as f64)));
            b.props.insert(PropKey::str("name"), readonly(name));
            b.props.insert(
                PropKey::str("prototype"),
                Property {
                    slot: Slot::Data {
                        value: Value::Object(proto.clone()),
                        writable: false,
                    },
                    enumerable: false,
                    configurable: false,
                },
            );
        }
        proto.borrow_mut().props.insert(PropKey::str("constructor"), Property::hidden(Value::Object(fo.clone())));
        Ok((fo, proto))
    }

    pub fn make_arguments(&mut self, args: &[Value], callee: Option<ObjRef>, strict: bool) -> ObjRef {
        let o = ObjRef::new(Object::new(Some(self.realm.object_proto.clone()), Kind::Arguments));
        {
            let mut b = o.borrow_mut();
            for (i, a) in args.iter().enumerate() {
                b.props.insert(index_key(i), Property::data(a.clone()));
            }
            b.props.insert(PropKey::str("length"), Property::hidden(Value::Number(args.len() as f64)));
            b.props.insert(
                PropKey::Sym(self.realm.sym_iterator.clone()),
                Property::hidden(Value::Object(self.realm.array_values.clone())),
            );
            if !strict && let Some(c) = callee {
                b.props.insert(PropKey::str("callee"), Property::hidden(Value::Object(c)));
            }
        }
        o
    }

    pub fn new_regexp(&mut self, source: JsStr, flags: JsStr) -> Result<Value, Value> {
        crate::builtins::regexp::create(self, source, flags, None)
    }

    // --------------------------------------------------- conversões

    pub fn to_primitive(&mut self, v: &Value, hint: Hint) -> Result<Value, Value> {
        let Value::Object(o) = v else { return Ok(v.clone()) };
        let key = PropKey::Sym(self.realm.sym_to_primitive.clone());
        let exotic = self.get_obj(o, &key)?;
        if !exotic.is_nullish() {
            let h = match hint {
                Hint::Default => "default",
                Hint::Number => "number",
                Hint::String => "string",
            };
            let r = self.call(&exotic, v.clone(), vec![Value::str(h)])?;
            if matches!(r, Value::Object(_)) {
                return Err(self.type_error("Cannot convert object to primitive value"));
            }
            return Ok(r);
        }
        let order = if hint == Hint::String { ["toString", "valueOf"] } else { ["valueOf", "toString"] };
        for name in order {
            let m = self.get_obj(o, &PropKey::str(name))?;
            if m.is_callable() {
                let r = self.call(&m, v.clone(), Vec::new())?;
                if !matches!(r, Value::Object(_)) {
                    return Ok(r);
                }
            }
        }
        Err(self.type_error("Cannot convert object to primitive value"))
    }

    pub fn to_string(&mut self, v: &Value) -> Result<JsStr, Value> {
        Ok(match v {
            Value::Undefined | Value::Empty => JsStr::new("undefined"),
            Value::Null => JsStr::new("null"),
            Value::Bool(b) => JsStr::new(if *b { "true" } else { "false" }),
            Value::Number(n) => JsStr::new(&number_to_string(*n)),
            Value::String(s) => s.clone(),
            Value::Symbol(_) => return Err(self.type_error("Cannot convert a Symbol value to a string")),
            Value::BigInt(b) => JsStr::new(&b.to_string_radix(10)),
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::String)?;
                return self.to_string(&p);
            }
        })
    }

    /// ToNumeric: Number ou BigInt.
    pub fn to_numeric(&mut self, v: &Value) -> Result<Value, Value> {
        match v {
            Value::Number(_) | Value::BigInt(_) => Ok(v.clone()),
            _ => {
                let p = self.to_primitive(v, Hint::Number)?;
                if let Value::BigInt(_) = p {
                    return Ok(p);
                }
                Ok(Value::Number(self.to_number(&p)?))
            }
        }
    }

    /// Operação aritmética/bit a bit com as regras de Number e BigInt.
    pub fn arith(&mut self, k: Arith, a: &Value, b: &Value) -> Result<Value, Value> {
        let x = self.to_numeric(a)?;
        let y = self.to_numeric(b)?;
        match (&x, &y) {
            (Value::Number(x), Value::Number(y)) => Ok(Value::Number(num_op(k, *x, *y))),
            (Value::BigInt(x), Value::BigInt(y)) => self.big_op(k, x, y),
            _ => Err(self.type_error("Cannot mix BigInt and other types, use explicit conversions")),
        }
    }

    fn big_op(&mut self, k: Arith, x: &crate::bigint::BigInt, y: &crate::bigint::BigInt) -> Result<Value, Value> {
        use crate::bigint::BigInt;
        let small = |v: &BigInt| -> Option<u64> {
            if v.is_negative() {
                return None;
            }
            let f = v.to_f64();
            if f > 1e9 { None } else { Some(f as u64) }
        };
        let r: BigInt = match k {
            Arith::Add => x.add(y),
            Arith::Sub => x.sub(y),
            Arith::Mul => x.mul(y),
            Arith::Div => match x.div(y) {
                Some(v) => v,
                None => return Err(self.range_error("Division by zero")),
            },
            Arith::Mod => match x.rem(y) {
                Some(v) => v,
                None => return Err(self.range_error("Division by zero")),
            },
            Arith::Exp => {
                if y.is_negative() {
                    return Err(self.range_error("Exponent must be non-negative"));
                }
                match small(y) {
                    Some(e) if e <= 10_000_000 => x.pow(e),
                    _ => return Err(self.range_error("Maximum BigInt size exceeded")),
                }
            }
            Arith::And => x.and(y),
            Arith::Or => x.or(y),
            Arith::Xor => x.xor(y),
            Arith::Shl | Arith::Shr => {
                let left = (k == Arith::Shl) != y.is_negative();
                let n = small(&if y.is_negative() { y.neg() } else { (*y).clone() });
                match (n, left) {
                    (Some(n), true) if n <= 1 << 30 => x.shl(n),
                    (Some(n), false) => x.shr(n),
                    (None, false) => {
                        if x.is_negative() { BigInt::from_i64(-1) } else { BigInt::zero() }
                    }
                    _ => return Err(self.range_error("Maximum BigInt size exceeded")),
                }
            }
            Arith::UShr => return Err(self.type_error("BigInts have no unsigned right shift, use >> instead")),
        };
        Ok(Value::BigInt(std::rc::Rc::new(r)))
    }

    pub fn to_rust_string(&mut self, v: &Value) -> Result<String, Value> {
        Ok(self.to_string(v)?.to_rust())
    }

    pub fn to_number(&mut self, v: &Value) -> Result<f64, Value> {
        Ok(match v {
            Value::Undefined | Value::Empty => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => *b as u8 as f64,
            Value::Number(n) => *n,
            Value::String(s) => string_to_number(s.units()),
            Value::Symbol(_) => return Err(self.type_error("Cannot convert a Symbol value to a number")),
            Value::BigInt(_) => return Err(self.type_error("Cannot convert a BigInt value to a number")),
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::Number)?;
                return self.to_number(&p);
            }
        })
    }

    pub fn to_integer(&mut self, v: &Value) -> Result<f64, Value> {
        Ok(crate::conv::to_integer(self.to_number(v)?))
    }

    pub fn to_property_key(&mut self, v: &Value) -> Result<PropKey, Value> {
        Ok(match v {
            Value::String(s) => PropKey::Str(s.clone()),
            Value::Symbol(s) => PropKey::Sym(s.clone()),
            Value::Number(n) => {
                if *n >= 0.0 && *n < 4294967296.0 && n.fract() == 0.0 {
                    PropKey::Str(JsStr::new(&(*n as u64).to_string()))
                } else {
                    PropKey::Str(JsStr::new(&number_to_string(*n)))
                }
            }
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::String)?;
                return self.to_property_key(&p);
            }
            _ => PropKey::Str(self.to_string(v)?),
        })
    }

    pub fn to_object(&mut self, v: &Value) -> Result<ObjRef, Value> {
        let (proto, kind) = match v {
            Value::Object(o) => return Ok(o.clone()),
            Value::Undefined | Value::Null | Value::Empty => {
                return Err(self.type_error("Cannot convert undefined or null to object"));
            }
            Value::Bool(b) => (self.realm.boolean_proto.clone(), Kind::Boolean(*b)),
            Value::Number(n) => (self.realm.number_proto.clone(), Kind::Number(*n)),
            Value::String(s) => (self.realm.string_proto.clone(), Kind::String(s.clone())),
            Value::Symbol(s) => (self.realm.symbol_proto.clone(), Kind::Symbol(s.clone())),
            Value::BigInt(b) => (self.realm.bigint_proto.clone(), Kind::BigIntBox(b.clone())),
        };
        Ok(ObjRef::new(Object::new(Some(proto), kind)))
    }

    /// ToLength para array-likes.
    pub fn length_of(&mut self, o: &ObjRef) -> Result<usize, Value> {
        match &o.borrow().kind {
            Kind::Array(v) => return Ok(v.len()),
            Kind::TypedArray(t) => return Ok(t.length),
            _ => {}
        }
        let l = self.get_obj(o, &PropKey::str("length"))?;
        let n = self.to_integer(&l)?;
        Ok(if n <= 0.0 { 0 } else { n.min(9007199254740991.0) as usize })
    }

    // ------------------------------------------------- propriedades

    pub fn get(&mut self, target: &Value, key: &PropKey) -> Result<Value, Value> {
        let proto = match target {
            Value::Object(o) => return self.get_with(o, key, target),
            Value::String(s) => {
                if let PropKey::Str(k) = key {
                    if k.eq_str("length") {
                        return Ok(Value::Number(s.len() as f64));
                    }
                    if let Some(i) = k.as_index()
                        && (i as usize) < s.len()
                    {
                        return Ok(Value::String(s.slice(i as usize, i as usize + 1)));
                    }
                }
                self.realm.string_proto.clone()
            }
            Value::Number(_) => self.realm.number_proto.clone(),
            Value::Bool(_) => self.realm.boolean_proto.clone(),
            Value::Symbol(_) => self.realm.symbol_proto.clone(),
            Value::BigInt(_) => self.realm.bigint_proto.clone(),
            Value::Undefined | Value::Null | Value::Empty => {
                let what = if matches!(target, Value::Null) { "null" } else { "undefined" };
                return Err(self.type_error(format!("Cannot read properties of {what} (reading '{}')", key_display(key))));
            }
        };
        self.get_with(&proto, key, target)
    }

    pub fn get_str(&mut self, target: &Value, name: &str) -> Result<Value, Value> {
        self.get(target, &PropKey::str(name))
    }

    pub fn get_obj(&mut self, o: &ObjRef, key: &PropKey) -> Result<Value, Value> {
        self.get_with(o, key, &Value::Object(o.clone()))
    }

    /// [[Get]] começando em `start`, com `receiver` como this dos getters.
    pub fn get_with(&mut self, start: &ObjRef, key: &PropKey, receiver: &Value) -> Result<Value, Value> {
        let mut cur = start.clone();
        loop {
            let (prop, next) = {
                let b = cur.borrow();
                if let Kind::Proxy { .. } = b.kind {
                    drop(b);
                    return self.proxy_get(&cur, key, receiver);
                }
                (b.own(key), b.proto.clone())
            };
            match prop {
                Some(p) => {
                    return match p.slot {
                        Slot::Data { value, .. } => Ok(value),
                        Slot::Accessor { get: Some(g), .. } => self.call_obj(&g, receiver.clone(), Vec::new()),
                        Slot::Accessor { get: None, .. } => Ok(Value::Undefined),
                    };
                }
                None => match next {
                    Some(n) => cur = n,
                    None => return Ok(Value::Undefined),
                },
            }
        }
    }

    /// Como get, mas distingue "não existe" (None).
    pub fn get_opt(&mut self, start: &ObjRef, key: &PropKey) -> Result<Option<Value>, Value> {
        let mut cur = start.clone();
        loop {
            let (prop, next) = {
                let b = cur.borrow();
                if let Kind::Proxy { .. } = b.kind {
                    drop(b);
                    if !self.proxy_has(&cur, key)? {
                        return Ok(None);
                    }
                    return Ok(Some(self.proxy_get(&cur, key, &Value::Object(start.clone()))?));
                }
                (b.own(key), b.proto.clone())
            };
            match prop {
                Some(p) => {
                    return match p.slot {
                        Slot::Data { value, .. } => Ok(Some(value)),
                        Slot::Accessor { get: Some(g), .. } => Ok(Some(self.call_obj(&g, Value::Object(start.clone()), Vec::new())?)),
                        Slot::Accessor { get: None, .. } => Ok(Some(Value::Undefined)),
                    };
                }
                None => match next {
                    Some(n) => cur = n,
                    None => return Ok(None),
                },
            }
        }
    }

    pub fn get_index(&mut self, obj: &Value, key: &Value) -> Result<Value, Value> {
        if let Value::Number(n) = key {
            let i = *n as usize;
            if i as f64 == *n {
                match obj {
                    Value::Object(o) => {
                        let b = o.borrow();
                        match &b.kind {
                            Kind::Array(v) => {
                                if let Some(x) = v.get(i)
                                    && !matches!(x, Value::Empty)
                                {
                                    return Ok(x.clone());
                                }
                            }
                            Kind::TypedArray(t) => return Ok(t.get(i).unwrap_or(Value::Undefined)),
                            _ => {}
                        }
                    }
                    Value::String(s) if i < s.len() => return Ok(Value::String(s.slice(i, i + 1))),
                    _ => {}
                }
            }
        }
        let k = self.to_property_key(key)?;
        self.get(obj, &k)
    }

    pub fn set_index(&mut self, obj: &Value, key: &Value, v: Value, strict: bool) -> Result<(), Value> {
        if let (Value::Object(o), Value::Number(n)) = (obj, key) {
            let i = *n as usize;
            if i as f64 == *n && matches!(o.borrow().kind, Kind::TypedArray(_)) {
                return self.typed_set(o, i, &v);
            }
            if i as f64 == *n {
                let mut b = o.borrow_mut();
                let frozen = b.frozen;
                let ext = b.extensible;
                if let Kind::Array(vec) = &mut b.kind
                    && !frozen
                {
                    if i < vec.len() && !matches!(vec[i], Value::Empty) {
                        vec[i] = v;
                        return Ok(());
                    }
                    if i == vec.len() && ext {
                        vec.push(v);
                        return Ok(());
                    }
                }
            }
        }
        let k = self.to_property_key(key)?;
        self.put(obj, k, v, strict)
    }

    /// Atribuição `obj[key] = v` (falha silenciosa fora do modo estrito).
    pub fn put(&mut self, target: &Value, key: PropKey, v: Value, strict: bool) -> Result<(), Value> {
        match target {
            Value::Object(o) => {
                let ok = self.set_with(o, key.clone(), v, target)?;
                if !ok && strict {
                    let d = self.describe(target);
                    return Err(self.type_error(format!("Cannot assign to read only property '{}' of object '{d}'", key_display(&key))));
                }
                Ok(())
            }
            Value::Undefined | Value::Null | Value::Empty => {
                let what = if matches!(target, Value::Null) { "null" } else { "undefined" };
                Err(self.type_error(format!("Cannot set properties of {what} (setting '{}')", key_display(&key))))
            }
            _ => {
                let proto = self.to_object(target)?.proto();
                let ok = match proto {
                    Some(p) => self.set_with(&p, key.clone(), v, target)?,
                    None => false,
                };
                if !ok && strict {
                    let d = self.describe(target);
                    return Err(self.type_error(format!("Cannot create property '{}' on {d}", key_display(&key))));
                }
                Ok(())
            }
        }
    }

    pub fn set_str(&mut self, target: &Value, name: &str, v: Value) -> Result<(), Value> {
        self.put(target, PropKey::str(name), v, true)
    }

    /// OrdinarySet: procura setter/somente-leitura na cadeia e grava em `receiver`.
    pub fn set_with(&mut self, start: &ObjRef, key: PropKey, v: Value, receiver: &Value) -> Result<bool, Value> {
        // caminho rápido: propriedade própria comum
        if let Value::Object(r) = receiver
            && r.ptr_eq(start)
        {
            let mut b = r.borrow_mut();
            if !matches!(b.kind, Kind::Array(_) | Kind::String(_))
                && let Some(Property {
                    slot: Slot::Data { value, writable: true },
                    ..
                }) = b.props.get_mut(&key)
            {
                *value = v;
                return Ok(true);
            }
        }
        let mut cur = start.clone();
        loop {
            let (prop, next) = {
                let b = cur.borrow();
                if let Kind::Proxy { .. } = b.kind {
                    drop(b);
                    return self.proxy_set(&cur, key, v, receiver);
                }
                (b.own(&key), b.proto.clone())
            };
            match prop {
                Some(Property {
                    slot: Slot::Data { writable, .. },
                    ..
                }) => {
                    if !writable {
                        return Ok(false);
                    }
                    break;
                }
                Some(Property {
                    slot: Slot::Accessor { set, .. },
                    ..
                }) => {
                    return match set {
                        Some(s) => {
                            self.call_obj(&s, receiver.clone(), vec![v])?;
                            Ok(true)
                        }
                        None => Ok(false),
                    };
                }
                None => match next {
                    Some(n) => cur = n,
                    None => break,
                },
            }
        }
        let Value::Object(r) = receiver else { return Ok(false) };
        if crate::builtins::proxy::is_proxy(r) {
            // receptor Proxy: [[GetOwnProperty]] e [[DefineOwnProperty]] passam pelas armadilhas
            use crate::builtins::object::Descriptor;
            let d = match self.get_own_property(r, &key)? {
                Some(Property {
                    slot: Slot::Data { writable: false, .. },
                    ..
                })
                | Some(Property {
                    slot: Slot::Accessor { .. },
                    ..
                }) => return Ok(false),
                Some(_) => Descriptor {
                    value: Some(v),
                    ..Default::default()
                },
                None => Descriptor::data(v, true),
            };
            return self.proxy_define(r, key, d);
        }
        self.set_own(r, key, v)
    }

    /// Grava uma propriedade própria de dados (criando se preciso).
    pub fn set_own(&mut self, obj: &ObjRef, key: PropKey, v: Value) -> Result<bool, Value> {
        if matches!(obj.borrow().kind, Kind::TypedArray(_))
            && let Some(i) = key.as_index()
        {
            self.typed_set(obj, i as usize, &v)?;
            return Ok(true);
        }
        {
            let mut b = obj.borrow_mut();
            let frozen = b.frozen;
            let ext = b.extensible;
            // índice com atributos especiais: o valor muda lá nas propriedades
            let in_props = matches!(b.kind, Kind::Array(_)) && b.props.contains(&key);
            if let Kind::Array(vec) = &mut b.kind {
                if let Some(i) = key.as_index()
                    && !in_props
                {
                    let i = i as usize;
                    if frozen {
                        return Ok(false);
                    }
                    if i < vec.len() {
                        if matches!(vec[i], Value::Empty) && !ext {
                            return Ok(false);
                        }
                        vec[i] = v;
                        return Ok(true);
                    }
                    if !ext {
                        return Ok(false);
                    }
                    if i < vec.len() + 1_000_000 {
                        vec.resize(i, Value::Empty);
                        vec.push(v);
                        return Ok(true);
                    }
                } else if let PropKey::Str(s) = &key
                    && s.eq_str("length")
                {
                    drop(b);
                    return self.set_array_length(obj, &v);
                }
            }
            match b.props.get_mut(&key) {
                Some(p) => {
                    return Ok(match &mut p.slot {
                        Slot::Data { value, writable } => {
                            if *writable {
                                *value = v;
                                true
                            } else {
                                false
                            }
                        }
                        Slot::Accessor { .. } => false,
                    });
                }
                None => {
                    if !b.extensible {
                        return Ok(false);
                    }
                    b.props.insert(key, Property::data(v));
                    Ok(true)
                }
            }
        }
    }

    pub fn set_array_length(&mut self, obj: &ObjRef, v: &Value) -> Result<bool, Value> {
        let n = self.to_number(v)?;
        if n < 0.0 || n != to_uint32(n) as f64 {
            return Err(self.range_error("Invalid array length"));
        }
        let mut b = obj.borrow_mut();
        if b.frozen {
            return Ok(false);
        }
        if let Kind::Array(vec) = &mut b.kind {
            let n = n as usize;
            if n > vec.len() + 50_000_000 {
                return Err(self.range_error("Invalid array length"));
            }
            vec.resize(n, Value::Empty);
        }
        Ok(true)
    }

    /// CreateDataProperty.
    pub fn define_data(&mut self, obj: &ObjRef, key: PropKey, v: Value) {
        if matches!(obj.borrow().kind, Kind::TypedArray(_))
            && let Some(i) = key.as_index()
        {
            let _ = self.typed_set(obj, i as usize, &v);
            return;
        }
        let mut b = obj.borrow_mut();
        if matches!(b.kind, Kind::Array(_))
            && let Some(i) = key.as_index()
        {
            // o elemento vai para o vetor: some a versão com atributos especiais
            if !b.props.is_empty() {
                b.props.remove(&key);
            }
            if let Kind::Array(vec) = &mut b.kind {
                let i = i as usize;
                if i < vec.len() {
                    vec[i] = v;
                    return;
                }
                if i < vec.len() + 1_000_000 {
                    vec.resize(i, Value::Empty);
                    vec.push(v);
                    return;
                }
            }
        }
        b.props.insert(key, Property::data(v));
    }

    pub fn has_property(&mut self, obj: &ObjRef, key: &PropKey) -> Result<bool, Value> {
        let mut cur = obj.clone();
        loop {
            let (has, next) = {
                let b = cur.borrow();
                if let Kind::Proxy { .. } = b.kind {
                    drop(b);
                    return self.proxy_has(&cur, key);
                }
                (b.own(key).is_some(), b.proto.clone())
            };
            if has {
                return Ok(true);
            }
            match next {
                Some(n) => cur = n,
                None => return Ok(false),
            }
        }
    }

    pub fn has_own(&mut self, obj: &ObjRef, key: &PropKey) -> Result<bool, Value> {
        Ok(self.get_own_property(obj, key)?.is_some())
    }

    /// [[Delete]]: false se a propriedade não pode ser apagada.
    pub fn delete_prop(&mut self, obj: &ObjRef, key: &PropKey) -> bool {
        if let Kind::TypedArray(t) = &obj.borrow().kind
            && let Some(i) = key.as_index()
        {
            return (i as usize) >= t.length;
        }
        let mut b = obj.borrow_mut();
        let frozen = b.frozen;
        if let Kind::Array(vec) = &mut b.kind {
            // buraco no vetor: o índice pode estar nas propriedades (acessor...)
            if let Some(i) = key.as_index()
                && let Some(x) = vec.get_mut(i as usize)
                && !matches!(x, Value::Empty)
            {
                if frozen {
                    return false;
                }
                *x = Value::Empty;
                return true;
            }
            if let PropKey::Str(s) = key
                && s.eq_str("length")
            {
                return false;
            }
        }
        if let Kind::String(s) = &b.kind {
            if let Some(i) = key.as_index()
                && (i as usize) < s.len()
            {
                return false;
            }
            if let PropKey::Str(n) = key
                && n.eq_str("length")
            {
                return false;
            }
        }
        match b.props.get(key) {
            Some(p) if !p.configurable => false,
            Some(_) => {
                b.props.remove(key);
                true
            }
            None => true,
        }
    }

    pub fn delete_value(&mut self, target: &Value, key: &PropKey, strict: bool) -> Result<bool, Value> {
        match target {
            Value::Object(o) => {
                let ok = if crate::builtins::proxy::is_proxy(o) { self.proxy_delete(o, key)? } else { self.delete_prop(o, key) };
                if !ok && strict {
                    let d = self.describe(target);
                    return Err(self.type_error(format!("Cannot delete property '{}' of {d}", key_display(key))));
                }
                Ok(ok)
            }
            Value::Undefined | Value::Null | Value::Empty => {
                Err(self.type_error(format!("Cannot convert undefined or null to object (deleting '{}')", key_display(key))))
            }
            _ => Ok(true),
        }
    }

    /// Chaves próprias (sem nomes privados).
    pub fn own_keys(&mut self, obj: &ObjRef) -> Result<Vec<PropKey>, Value> {
        if crate::builtins::proxy::is_proxy(obj) {
            return self.proxy_own_keys(obj);
        }
        Ok(obj
            .borrow()
            .own_keys()
            .into_iter()
            .filter(|k| !matches!(k, PropKey::Sym(s) if s.is_private()))
            .collect())
    }

    /// Copia propriedades próprias enumeráveis (spread de objeto, Object.assign).
    pub fn copy_data_props(&mut self, target: &ObjRef, src: &Value, excluded: &[PropKey]) -> Result<(), Value> {
        let src_obj = match src {
            Value::Undefined | Value::Null | Value::Empty => return Ok(()),
            Value::Object(o) => o.clone(),
            other => self.to_object(other)?,
        };
        for k in self.own_keys(&src_obj)? {
            if excluded.contains(&k) {
                continue;
            }
            if !self.is_enumerable_own(&src_obj, &k)? {
                continue;
            }
            let v = self.get_with(&src_obj, &k, src)?;
            self.define_data(target, k, v);
        }
        Ok(())
    }

    // --------------------------------------------------------- globais

    pub fn get_global(&mut self, name: &JsStr) -> Result<Value, Value> {
        if let Some(cell) = self.lex.get(name) {
            let v = cell.borrow().value.clone();
            if matches!(v, Value::Empty) {
                return Err(self.reference_error(format!("Cannot access '{name}' before initialization")));
            }
            return Ok(v);
        }
        let g = self.realm.global.clone();
        let key = PropKey::Str(name.clone());
        {
            let b = g.borrow();
            if let Some(Property {
                slot: Slot::Data { value, .. },
                ..
            }) = b.props.get(&key)
            {
                return Ok(value.clone());
            }
        }
        match self.get_opt(&g, &key)? {
            Some(v) => Ok(v),
            None => match self.global_miss.and_then(|f| f(self, name)) {
                Some(v) => Ok(v),
                None => Err(self.reference_error(format!("{name} is not defined"))),
            },
        }
    }

    pub fn set_global(&mut self, name: &JsStr, v: Value, strict: bool) -> Result<(), Value> {
        if let Some(cell) = self.lex.get(name).cloned() {
            let (empty, is_const) = {
                let b = cell.borrow();
                (matches!(b.value, Value::Empty), b.is_const)
            };
            if empty {
                return Err(self.reference_error(format!("Cannot access '{name}' before initialization")));
            }
            if is_const {
                return Err(self.type_error("Assignment to constant variable."));
            }
            cell.borrow_mut().value = v;
            return Ok(());
        }
        let g = self.realm.global.clone();
        let key = PropKey::Str(name.clone());
        if strict && !self.has_property(&g, &key)? {
            return Err(self.reference_error(format!("{name} is not defined")));
        }
        self.put(&Value::Object(g), key, v, strict)
    }

    // -------------------------------------------------------- operadores

    pub fn add(&mut self, a: &Value, b: &Value) -> Result<Value, Value> {
        let pa = self.to_primitive(a, Hint::Default)?;
        let pb = self.to_primitive(b, Hint::Default)?;
        if matches!(pa, Value::String(_)) || matches!(pb, Value::String(_)) {
            let sa = self.to_string(&pa)?;
            let sb = self.to_string(&pb)?;
            return Ok(Value::String(sa.concat(&sb)));
        }
        self.arith(Arith::Add, &pa, &pb)
    }

    /// Comparação abstrata `a < b` (None = indefinido, por NaN).
    pub fn less_than(&mut self, a: &Value, b: &Value, left_first: bool) -> Result<Option<bool>, Value> {
        let (pa, pb) = if left_first {
            let pa = self.to_primitive(a, Hint::Number)?;
            let pb = self.to_primitive(b, Hint::Number)?;
            (pa, pb)
        } else {
            let pb = self.to_primitive(b, Hint::Number)?;
            let pa = self.to_primitive(a, Hint::Number)?;
            (pa, pb)
        };
        if let (Value::String(x), Value::String(y)) = (&pa, &pb) {
            return Ok(Some(x.units() < y.units()));
        }
        // BigInt contra string/número: comparação matemática exata
        let as_big = |v: &Value| match v {
            Value::String(s) => crate::bigint::BigInt::parse_js(&s.to_rust()).map(|b| Value::BigInt(std::rc::Rc::new(b))),
            other => Some(other.clone()),
        };
        if matches!(pa, Value::BigInt(_)) || matches!(pb, Value::BigInt(_)) {
            let (Some(xa), Some(xb)) = (as_big(&pa), as_big(&pb)) else { return Ok(None) };
            let xa = self.to_numeric(&xa)?;
            let xb = self.to_numeric(&xb)?;
            return Ok(match (&xa, &xb) {
                (Value::BigInt(a), Value::BigInt(b)) => Some(a.cmp(b) == std::cmp::Ordering::Less),
                (Value::BigInt(a), Value::Number(n)) => a.cmp_f64(*n).map(|o| o == std::cmp::Ordering::Less),
                (Value::Number(n), Value::BigInt(b)) => b.cmp_f64(*n).map(|o| o == std::cmp::Ordering::Greater),
                _ => None,
            });
        }
        let x = self.to_number(&pa)?;
        let y = self.to_number(&pb)?;
        if x.is_nan() || y.is_nan() {
            return Ok(None);
        }
        Ok(Some(x < y))
    }

    pub fn loose_equals(&mut self, a: &Value, b: &Value) -> Result<bool, Value> {
        Ok(match (a, b) {
            (Value::Undefined | Value::Null | Value::Empty, Value::Undefined | Value::Null | Value::Empty) => true,
            (Value::Number(_), Value::Number(_))
            | (Value::String(_), Value::String(_))
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Symbol(_), Value::Symbol(_))
            | (Value::Object(_), Value::Object(_)) => strict_equals(a, b),
            (Value::Number(x), Value::String(s)) | (Value::String(s), Value::Number(x)) => *x == string_to_number(s.units()),
            (Value::BigInt(x), Value::BigInt(y)) => x == y,
            (Value::BigInt(b), Value::Number(n)) | (Value::Number(n), Value::BigInt(b)) => b.cmp_f64(*n) == Some(std::cmp::Ordering::Equal),
            (Value::BigInt(b), Value::String(s)) | (Value::String(s), Value::BigInt(b)) => {
                crate::bigint::BigInt::parse_js(&s.to_rust()).is_some_and(|x| x == **b)
            }
            (Value::Bool(x), _) => return self.loose_equals(&Value::Number(*x as u8 as f64), b),
            (_, Value::Bool(y)) => return self.loose_equals(a, &Value::Number(*y as u8 as f64)),
            (Value::Number(_) | Value::String(_) | Value::Symbol(_) | Value::BigInt(_), Value::Object(_)) => {
                let pb = self.to_primitive(b, Hint::Default)?;
                return self.loose_equals(a, &pb);
            }
            (Value::Object(_), Value::Number(_) | Value::String(_) | Value::Symbol(_) | Value::BigInt(_)) => {
                let pa = self.to_primitive(a, Hint::Default)?;
                return self.loose_equals(&pa, b);
            }
            _ => false,
        })
    }

    pub fn instance_of(&mut self, v: &Value, target: &Value) -> Result<bool, Value> {
        let Value::Object(t) = target else {
            return Err(self.type_error("Right-hand side of 'instanceof' is not an object"));
        };
        let key = PropKey::Sym(self.realm.sym_has_instance.clone());
        let h = self.get_obj(t, &key)?;
        if let Value::Object(ho) = &h {
            if ho.ptr_eq(&self.realm.function_has_instance) {
                return self.ordinary_has_instance(t, v);
            }
            let r = self.call(&h, target.clone(), vec![v.clone()])?;
            return Ok(truthy(&r));
        }
        if !t.is_callable() {
            return Err(self.type_error("Right-hand side of 'instanceof' is not callable"));
        }
        self.ordinary_has_instance(t, v)
    }

    pub fn ordinary_has_instance(&mut self, c: &ObjRef, v: &Value) -> Result<bool, Value> {
        if !c.is_callable() {
            return Ok(false);
        }
        let bound = match &c.borrow().kind {
            Kind::Function(Func::Bound { target, .. }) => Some(target.clone()),
            _ => None,
        };
        if let Some(t) = bound {
            return self.instance_of(v, &Value::Object(t));
        }
        let Value::Object(o) = v else { return Ok(false) };
        let proto = self.get_obj(c, &PropKey::str("prototype"))?;
        let Value::Object(proto) = proto else {
            return Err(self.type_error("Function has non-object prototype in instanceof check"));
        };
        let mut cur = self.get_prototype_of(o)?;
        while let Some(p) = cur {
            if p.ptr_eq(&proto) {
                return Ok(true);
            }
            cur = self.get_prototype_of(&p)?;
        }
        Ok(false)
    }

    // --------------------------------------------------------- iteração

    pub fn get_iterator(&mut self, v: &Value) -> Result<ObjRef, Value> {
        let key = PropKey::Sym(self.realm.sym_iterator.clone());
        let m = self.get(v, &key)?;
        if !m.is_callable() {
            let d = match v {
                Value::Number(_) | Value::Bool(_) | Value::Symbol(_) => format!("{} {}", crate::vm::typeof_str(v), self.describe(v)),
                Value::Object(o) if o.is_callable() => "function".into(),
                Value::Object(_) => "object".into(),
                _ => self.describe(v),
            };
            return Err(self.type_error(format!("{d} is not iterable (cannot read property Symbol(Symbol.iterator))")));
        }
        let it = self.call(&m, v.clone(), Vec::new())?;
        if !matches!(it, Value::Object(_)) {
            return Err(self.type_error("Result of the Symbol.iterator method is not an object"));
        }
        let next = self.get_str(&it, "next")?;
        Ok(ObjRef::new(Object::new(
            None,
            Kind::IterRecord {
                iter: it,
                next,
                done: false,
                sync_wrapped: false,
            },
        )))
    }

    /// Próximo valor do iterador (None quando acaba).
    pub fn iter_step(&mut self, rec: &ObjRef) -> Result<Option<Value>, Value> {
        let (iter, next) = match &rec.borrow().kind {
            Kind::IterRecord { iter, next, done, .. } => {
                if *done {
                    return Ok(None);
                }
                (iter.clone(), next.clone())
            }
            _ => return Ok(None),
        };
        if let (Value::Object(it), Value::Object(nx)) = (&iter, &next)
            && nx.ptr_eq(&self.realm.array_iter_next)
            && let Some(r) = self.array_iter_fast(it)
        {
            if r.is_none() {
                set_done(rec);
            }
            return Ok(r);
        }
        let res = self.call(&next, iter, Vec::new())?;
        if !matches!(res, Value::Object(_)) {
            let d = self.describe(&res);
            return Err(self.type_error(format!("Iterator result {d} is not an object")));
        }
        let done = self.get_str(&res, "done")?;
        if truthy(&done) {
            set_done(rec);
            return Ok(None);
        }
        Ok(Some(self.get_str(&res, "value")?))
    }

    /// Passo de um iterador de array sem chamar `next` (None = use o caminho geral).
    pub fn array_iter_fast(&mut self, it: &ObjRef) -> Option<Option<Value>> {
        let mut b = it.borrow_mut();
        let Kind::ArrayIter { target, index, kind } = &mut b.kind else { return None };
        let Value::Object(t) = target else { return None };
        let tb = t.0.try_borrow().ok()?;
        let Kind::Array(v) = &tb.kind else { return None };
        if *index >= v.len() {
            *index = usize::MAX;
            return Some(None);
        }
        let i = *index;
        *index += 1;
        let val = match v[i] {
            Value::Empty => Value::Undefined,
            ref x => x.clone(),
        };
        Some(Some(match kind {
            IterKind::Values => val,
            IterKind::Keys => Value::Number(i as f64),
            IterKind::Entries => {
                let pair = vec![Value::Number(i as f64), val];
                Value::Object(ObjRef::new(Object::new(Some(self.realm.array_proto.clone()), Kind::Array(pair))))
            }
        }))
    }

    pub fn iter_close(&mut self, rec: &ObjRef) -> Result<(), Value> {
        let iter = match &rec.borrow().kind {
            Kind::IterRecord { iter, done, .. } => {
                if *done {
                    return Ok(());
                }
                iter.clone()
            }
            _ => return Ok(()),
        };
        set_done(rec);
        let ret = self.get_str(&iter, "return")?;
        if ret.is_nullish() {
            return Ok(());
        }
        let r = self.call(&ret, iter, Vec::new())?;
        if !matches!(r, Value::Object(_)) {
            return Err(self.type_error("Iterator result is not an object"));
        }
        Ok(())
    }

    /// Todos os valores de um iterável (spread, Array.from...).
    pub fn iterate_to_vec(&mut self, v: &Value) -> Result<Vec<Value>, Value> {
        // caminho rápido: array comum com o iterador padrão
        if let Value::Object(o) = v
            && self.has_default_array_iterator(o)
            && let Kind::Array(items) = &o.borrow().kind
        {
            return Ok(items
                .iter()
                .map(|x| match x {
                    Value::Empty => Value::Undefined,
                    x => x.clone(),
                })
                .collect());
        }
        let rec = self.get_iterator(v)?;
        let mut out = Vec::new();
        while let Some(x) = self.iter_step(&rec)? {
            out.push(x);
        }
        Ok(out)
    }

    pub fn has_default_array_iterator(&self, o: &ObjRef) -> bool {
        let b = o.borrow();
        if !matches!(b.kind, Kind::Array(_)) {
            return false;
        }
        let key = PropKey::Sym(self.realm.sym_iterator.clone());
        if b.props.contains(&key) {
            return false;
        }
        let Some(p) = &b.proto else { return false };
        if !p.ptr_eq(&self.realm.array_proto) {
            return false;
        }
        matches!(
            self.realm.array_proto.borrow().props.get(&key),
            Some(Property { slot: Slot::Data { value: Value::Object(f), .. }, .. }) if f.ptr_eq(&self.realm.array_values)
        )
    }

    pub fn for_in_iterator(&mut self, v: &Value) -> Result<ObjRef, Value> {
        let mut keys = Vec::new();
        let target = if v.is_nullish() {
            None
        } else {
            let o = self.to_object(v)?;
            let mut seen: HashSet<PropKey> = HashSet::new();
            let mut cur = Some(o.clone());
            while let Some(c) = cur {
                let ks = self.own_keys(&c)?;
                for k in ks {
                    if let PropKey::Str(s) = &k
                        && seen.insert(k.clone())
                        && self.is_enumerable_own(&c, &k)?
                    {
                        keys.push(s.clone());
                    }
                }
                cur = self.get_prototype_of(&c)?;
            }
            Some(o)
        };
        Ok(ObjRef::new(Object::new(None, Kind::ForIn { keys, index: 0, target })))
    }

    pub fn for_in_next(&mut self, it: &ObjRef) -> Option<JsStr> {
        loop {
            let (key, target) = {
                let mut b = it.borrow_mut();
                let Kind::ForIn { keys, index, target } = &mut b.kind else { return None };
                if *index >= keys.len() {
                    return None;
                }
                *index += 1;
                (keys[*index - 1].clone(), target.clone())
            };
            // propriedades apagadas durante o laço são puladas
            if let Some(t) = &target
                && !self.has_property(t, &PropKey::Str(key.clone())).unwrap_or(true)
            {
                continue;
            }
            return Some(key);
        }
    }

    /// Grava no elemento `i` de um typed array (convertendo o valor).
    pub fn typed_set(&mut self, o: &ObjRef, i: usize, v: &Value) -> Result<(), Value> {
        let is_big = match &o.borrow().kind {
            Kind::TypedArray(t) => t.kind.is_bigint(),
            _ => return Ok(()),
        };
        if is_big {
            let b = crate::builtins::bigint::to_bigint(self, v)?;
            if let Kind::TypedArray(t) = &o.borrow().kind {
                t.set_bigint(i, &b);
            }
        } else {
            let n = match v {
                Value::Number(n) => *n,
                _ => self.to_number(v)?,
            };
            if let Kind::TypedArray(t) = &o.borrow().kind {
                t.set_number(i, n);
            }
        }
        Ok(())
    }

    /// Array.prototype.includes / Map usam SameValueZero.
    pub fn same_value_zero(&self, a: &Value, b: &Value) -> bool {
        same_value_zero(a, b)
    }
}

fn set_done(rec: &ObjRef) {
    if let Kind::IterRecord { done, .. } = &mut rec.borrow_mut().kind {
        *done = true;
    }
}

/// Propriedade de dados na cadeia, sem chamar getters (para mensagens).
pub fn data_in_chain(o: &ObjRef, name: &str) -> Option<Value> {
    let key = PropKey::str(name);
    let mut cur = Some(o.clone());
    let mut depth = 0;
    while let Some(c) = cur {
        if let Some(Property {
            slot: Slot::Data { value, .. },
            ..
        }) = c.borrow().own(&key)
        {
            return Some(value);
        }
        depth += 1;
        if depth > 50 {
            return None;
        }
        cur = c.proto();
    }
    None
}

pub fn readonly(v: Value) -> Property {
    Property {
        slot: Slot::Data { value: v, writable: false },
        enumerable: false,
        configurable: true,
    }
}
