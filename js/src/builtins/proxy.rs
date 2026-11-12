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

