//! Map, Set, WeakMap, WeakSet, WeakRef e FinalizationRegistry.

use super::Realm;
use super::array::iter_result;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let mp = &r.map_proto;
    let mc = r.ctor("Map", 0, map_ctor, mp);
    r.method(&mc, "groupBy", 2, map_group_by);
    r.getter_sym(&mc, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.method(mp, "get", 1, map_get);
    r.method(mp, "set", 2, map_set);
    r.method(mp, "has", 1, coll_has);
    r.method(mp, "delete", 1, coll_delete);
    r.method(mp, "clear", 0, coll_clear);
    r.method(mp, "forEach", 1, coll_for_each);
    r.method(mp, "keys", 0, coll_keys);
    r.method(mp, "values", 0, coll_values);
    let entries = r.method(mp, "entries", 0, coll_entries);
    mp.borrow_mut().props.insert(PropKey::Sym(r.sym_iterator.clone()), Property::hidden(Value::Object(entries)));
    r.getter(mp, "size", coll_size);
    r.tag(mp, "Map");

    let sp = &r.set_proto;
    let sc = r.ctor("Set", 0, set_ctor, sp);
    r.getter_sym(&sc, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.method(sp, "add", 1, set_add);
    r.method(sp, "has", 1, coll_has);
    r.method(sp, "delete", 1, coll_delete);
    r.method(sp, "clear", 0, coll_clear);
    r.method(sp, "forEach", 1, coll_for_each);
    r.method(sp, "entries", 0, coll_entries);
    let values = r.method(sp, "values", 0, coll_values);
    sp.borrow_mut().props.insert(PropKey::str("keys"), Property::hidden(Value::Object(values.clone())));
    sp.borrow_mut().props.insert(PropKey::Sym(r.sym_iterator.clone()), Property::hidden(Value::Object(values)));
    r.getter(sp, "size", coll_size);
    r.tag(sp, "Set");

    for ip in [&r.map_iter_proto, &r.set_iter_proto] {
        r.method(ip, "next", 0, iter_next);
    }
    r.tag(&r.map_iter_proto, "Map Iterator");
    r.tag(&r.set_iter_proto, "Set Iterator");

    let wm = &r.weakmap_proto;
    r.ctor("WeakMap", 0, weakmap_ctor, wm);
    r.method(wm, "get", 1, weak_get);
    r.method(wm, "set", 2, weakmap_set);
    r.method(wm, "has", 1, weak_has);
    r.method(wm, "delete", 1, weak_delete);
    r.tag(wm, "WeakMap");

    let ws = &r.weakset_proto;
    r.ctor("WeakSet", 0, weakset_ctor, ws);
    r.method(ws, "add", 1, weakset_add);
    r.method(ws, "has", 1, weak_has);
    r.method(ws, "delete", 1, weak_delete);
    r.tag(ws, "WeakSet");

    let wr = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.ctor("WeakRef", 1, weakref_ctor, &wr);
    r.method(&wr, "deref", 0, weakref_deref);
    r.tag(&wr, "WeakRef");

    let fr = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.ctor("FinalizationRegistry", 1, finreg_ctor, &fr);
    r.method(&fr, "register", 2, noop);
    r.method(&fr, "unregister", 1, noop_false);
    r.tag(&fr, "FinalizationRegistry");
}

fn noop(_vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Undefined)
}

fn noop_false(_vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(false))
}

fn require_new(vm: &mut Vm, a: &CallArgs, name: &str) -> Result<ObjRef, Value> {
    match &a.new_target {
        Some(nt) => Ok(nt.clone()),
        None => Err(vm.type_error(format!("Constructor {name} requires 'new'"))),
    }
}

fn new_coll(vm: &mut Vm, nt: &ObjRef, default: &ObjRef, set: bool) -> Result<ObjRef, Value> {
    let proto = vm.proto_from_ctor(Some(nt), default)?;
    let kind = if set { Kind::Set(Collection::default()) } else { Kind::Map(Collection::default()) };
    Ok(ObjRef::new(Object::new(Some(proto), kind)))
}

/// Preenche um Map a partir de um iterável de pares, chamando `adder`.
fn fill_from(vm: &mut Vm, o: &ObjRef, iterable: &Value, adder_name: &str, pairs: bool) -> Result<(), Value> {
    if iterable.is_nullish() {
        return Ok(());
    }
    let ov = Value::Object(o.clone());
    let adder = vm.get_str(&ov, adder_name)?;
    if !adder.is_callable() {
        return Err(vm.type_error(format!("'{adder_name}' is not a function")));
    }
    let items = vm.iterate_to_vec(iterable)?;
    for item in items {
        if pairs {
            if !matches!(item, Value::Object(_)) {
                let d = vm.describe(&item);
                return Err(vm.type_error(format!("Iterator value {d} is not an entry object")));
            }
            let k = vm.get_index(&item, &Value::Number(0.0))?;
            let v = vm.get_index(&item, &Value::Number(1.0))?;
            vm.call(&adder, ov.clone(), vec![k, v])?;
        } else {
            vm.call(&adder, ov.clone(), vec![item])?;
        }
    }
    Ok(())
}

fn map_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "Map")?;
    let def = vm.realm.map_proto.clone();
    let o = new_coll(vm, &nt, &def, false)?;
    fill_from(vm, &o, &a.arg(0), "set", true)?;
    Ok(Value::Object(o))
}

fn set_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "Set")?;
    let def = vm.realm.set_proto.clone();
    let o = new_coll(vm, &nt, &def, true)?;
    fill_from(vm, &o, &a.arg(0), "add", false)?;
    Ok(Value::Object(o))
}

fn weakmap_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "WeakMap")?;
    let def = vm.realm.weakmap_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let o = ObjRef::new(Object::new(Some(proto), Kind::WeakMap(WeakColl::default())));
    fill_from(vm, &o, &a.arg(0), "set", true)?;
    Ok(Value::Object(o))
}

fn weakset_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "WeakSet")?;
    let def = vm.realm.weakset_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let o = ObjRef::new(Object::new(Some(proto), Kind::WeakSet(WeakColl::default())));
    fill_from(vm, &o, &a.arg(0), "add", false)?;
    Ok(Value::Object(o))
}

