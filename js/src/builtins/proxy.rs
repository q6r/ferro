//! Proxy: objetos cujas operações internas chamam "armadilhas" do handler.

use super::object::{Descriptor, define_own_property, from_property, to_descriptor};
use super::{Realm, native_data, set_native_data};
use crate::value::*;
use crate::vm::{Vm, truthy};

pub fn install(r: &Realm) {
    let c = super::native_fn(&r.function_proto, "Proxy", 2, proxy_ctor, true);
    r.global.borrow_mut().props.insert(PropKey::str("Proxy"), Property::hidden(Value::Object(c.clone())));
    r.method(&c, "revocable", 2, revocable);
}

fn create(vm: &mut Vm, target: &Value, handler: &Value) -> Result<ObjRef, Value> {
    let (Value::Object(t), Value::Object(h)) = (target, handler) else {
        return Err(vm.type_error("Cannot create proxy with a non-object as target or handler"));
    };
    Ok(ObjRef::new(Object::new(
        None,
        Kind::Proxy {
            target: Some(t.clone()),
            handler: Some(h.clone()),
        },
    )))
}

fn proxy_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if a.new_target.is_none() {
        return Err(vm.type_error("Constructor Proxy requires 'new'"));
    }
    Ok(Value::Object(create(vm, &a.arg(0), &a.arg(1))?))
}

fn revocable(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let p = create(vm, &a.arg(0), &a.arg(1))?;
    let revoke = vm.realm.native("", 0, revoke);
    set_native_data(&revoke, Value::Object(p.clone()));
    let o = vm.new_object();
    vm.define_data(&o, PropKey::str("proxy"), Value::Object(p));
    vm.define_data(&o, PropKey::str("revoke"), Value::Object(revoke));
    Ok(Value::Object(o))
}

fn revoke(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(p) = native_data(&a.callee)
        && let Kind::Proxy { target, handler } = &mut p.borrow_mut().kind
    {
        *target = None;
        *handler = None;
    }
    Ok(Value::Undefined)
}

pub fn is_proxy(o: &ObjRef) -> bool {
    matches!(o.borrow().kind, Kind::Proxy { .. })
}

impl Vm {
    /// (alvo, handler) de um Proxy; erro se foi revogado.
    fn proxy_parts(&mut self, o: &ObjRef, op: &str) -> Result<(ObjRef, ObjRef), Value> {
        let parts = match &o.borrow().kind {
            Kind::Proxy { target: Some(t), handler: Some(h) } => Some((t.clone(), h.clone())),
            _ => None,
        };
        parts.ok_or_else(|| self.type_error(format!("Cannot perform '{op}' on a proxy that has been revoked")))
    }

    /// A armadilha `name` do handler (None se não foi definida).
    fn trap(&mut self, h: &ObjRef, name: &str) -> Result<Option<Value>, Value> {
        let t = self.get_obj(h, &PropKey::str(name))?;
        if t.is_nullish() {
            return Ok(None);
        }
        if !t.is_callable() {
            return Err(self.type_error(format!("'{name}' on proxy: trap is not a function")));
        }
        Ok(Some(t))
    }

    pub fn proxy_get(&mut self, o: &ObjRef, key: &PropKey, receiver: &Value) -> Result<Value, Value> {
        let (t, h) = self.proxy_parts(o, "get")?;
        match self.trap(&h, "get")? {
            Some(f) => self.call(&f, Value::Object(h), vec![Value::Object(t), key.to_value(), receiver.clone()]),
            None => self.get_with(&t, key, receiver),
        }
    }

    pub fn proxy_set(&mut self, o: &ObjRef, key: PropKey, v: Value, receiver: &Value) -> Result<bool, Value> {
        let (t, h) = self.proxy_parts(o, "set")?;
        match self.trap(&h, "set")? {
            Some(f) => {
                let r = self.call(&f, Value::Object(h), vec![Value::Object(t), key.to_value(), v, receiver.clone()])?;
                Ok(truthy(&r))
            }
            None => self.set_with(&t, key, v, receiver),
        }
    }

    pub fn proxy_has(&mut self, o: &ObjRef, key: &PropKey) -> Result<bool, Value> {
        let (t, h) = self.proxy_parts(o, "has")?;
        match self.trap(&h, "has")? {
            Some(f) => {
                let r = self.call(&f, Value::Object(h), vec![Value::Object(t), key.to_value()])?;
                Ok(truthy(&r))
            }
            None => self.has_property(&t, key),
        }
    }

    pub fn proxy_delete(&mut self, o: &ObjRef, key: &PropKey) -> Result<bool, Value> {
        let (t, h) = self.proxy_parts(o, "deleteProperty")?;
        match self.trap(&h, "deleteProperty")? {
            Some(f) => {
                let r = self.call(&f, Value::Object(h), vec![Value::Object(t), key.to_value()])?;
                Ok(truthy(&r))
            }
            None => {
                if is_proxy(&t) {
                    return self.proxy_delete(&t, key);
                }
                Ok(self.delete_prop(&t, key))
            }
        }
    }

    pub fn proxy_own_keys(&mut self, o: &ObjRef) -> Result<Vec<PropKey>, Value> {
        let (t, h) = self.proxy_parts(o, "ownKeys")?;
        match self.trap(&h, "ownKeys")? {
            Some(f) => {
                let r = self.call(&f, Value::Object(h), vec![Value::Object(t)])?;
                let list = super::function::list_from_array_like(self, &r)?;
                let mut out = Vec::with_capacity(list.len());
                for v in list {
                    match v {
                        Value::String(s) => out.push(PropKey::Str(s)),
                        Value::Symbol(s) => out.push(PropKey::Sym(s)),
                        other => {
                            let d = self.describe(&other);
                            return Err(self.type_error(format!("{d} is not a valid property name")));
                        }
                    }
                }
                Ok(out)
            }
            None => self.own_keys(&t),
        }
    }

    pub fn proxy_get_own_property(&mut self, o: &ObjRef, key: &PropKey) -> Result<Option<Property>, Value> {
        let (t, h) = self.proxy_parts(o, "getOwnPropertyDescriptor")?;
        match self.trap(&h, "getOwnPropertyDescriptor")? {
            Some(f) => {
                let r = self.call(&f, Value::Object(h), vec![Value::Object(t), key.to_value()])?;
                if r.is_undefined() {
                    return Ok(None);
                }
                let d = to_descriptor(self, &r)?;
                let slot = if d.is_accessor() {
                    Slot::Accessor {
                        get: d.get.clone().flatten(),
                        set: d.set.clone().flatten(),
                    }
                } else {
                    Slot::Data {
                        value: d.value.clone().unwrap_or_default(),
                        writable: d.writable.unwrap_or(false),
                    }
                };
                Ok(Some(Property {
                    slot,
                    enumerable: d.enumerable.unwrap_or(false),
                    configurable: d.configurable.unwrap_or(false),
                }))
            }
            None => self.get_own_property(&t, key),
        }
    }

    pub fn proxy_define(&mut self, o: &ObjRef, key: PropKey, d: Descriptor) -> Result<bool, Value> {
        let (t, h) = self.proxy_parts(o, "defineProperty")?;
        match self.trap(&h, "defineProperty")? {
            Some(f) => {
                let p = Property {
                    slot: if d.is_accessor() {
                        Slot::Accessor {
                            get: d.get.clone().flatten(),
                            set: d.set.clone().flatten(),
                        }
                    } else {
                        Slot::Data {
                            value: d.value.clone().unwrap_or_default(),
                            writable: d.writable.unwrap_or(false),
                        }
                    },
                    enumerable: d.enumerable.unwrap_or(false),
                    configurable: d.configurable.unwrap_or(false),
                };
                let desc = from_property(self, &p);
                let r = self.call(&f, Value::Object(h), vec![Value::Object(t), key.to_value(), desc])?;
                Ok(truthy(&r))
            }
            None => define_own_property(self, &t, key, d),
        }
    }

    pub fn proxy_get_proto(&mut self, o: &ObjRef) -> Result<Option<ObjRef>, Value> {
        let (t, h) = self.proxy_parts(o, "getPrototypeOf")?;
        match self.trap(&h, "getPrototypeOf")? {
            Some(f) => match self.call(&f, Value::Object(h), vec![Value::Object(t)])? {
                Value::Object(p) => Ok(Some(p)),
                Value::Null => Ok(None),
                _ => Err(self.type_error("'getPrototypeOf' on proxy: trap returned neither object nor null")),
            },
            None => self.get_prototype_of(&t),
        }
    }

    pub fn proxy_set_proto(&mut self, o: &ObjRef, p: Option<ObjRef>) -> Result<bool, Value> {
        let (t, h) = self.proxy_parts(o, "setPrototypeOf")?;
        match self.trap(&h, "setPrototypeOf")? {
            Some(f) => {
                let pv = p.map(Value::Object).unwrap_or(Value::Null);
                let r = self.call(&f, Value::Object(h), vec![Value::Object(t), pv])?;
                Ok(truthy(&r))
            }
            None => super::object::set_proto_checked(self, &t, p),
        }
    }

    pub fn proxy_is_extensible(&mut self, o: &ObjRef) -> Result<bool, Value> {
        let (t, h) = self.proxy_parts(o, "isExtensible")?;
        match self.trap(&h, "isExtensible")? {
            Some(f) => Ok(truthy(&self.call(&f, Value::Object(h), vec![Value::Object(t)])?)),
            None => self.is_extensible(&t),
        }
    }

    pub fn proxy_prevent_extensions(&mut self, o: &ObjRef) -> Result<bool, Value> {
        let (t, h) = self.proxy_parts(o, "preventExtensions")?;
        match self.trap(&h, "preventExtensions")? {
            Some(f) => Ok(truthy(&self.call(&f, Value::Object(h), vec![Value::Object(t)])?)),
            None => self.prevent_extensions(&t),
        }
    }

    pub fn proxy_call(&mut self, o: &ObjRef, this: Value, args: Vec<Value>) -> Result<Value, Value> {
        let (t, h) = self.proxy_parts(o, "apply")?;
        match self.trap(&h, "apply")? {
            Some(f) => {
                let arr = self.array_value(args);
                self.call(&f, Value::Object(h), vec![Value::Object(t), this, arr])
            }
            None => self.call_obj(&t, this, args),
        }
    }

