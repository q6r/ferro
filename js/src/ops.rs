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

