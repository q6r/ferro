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
    let set = r.method(mp, "set", 2, map_set);
    // o construtor guarda o `set` original (atalho quando não foi trocado)
    super::set_native_data(&mc, Value::Object(set));
    r.method(mp, "has", 1, map_has);
    r.method(mp, "delete", 1, map_delete);
    r.method(mp, "clear", 0, map_clear);
    r.method(mp, "forEach", 1, map_for_each);
    r.method(mp, "getOrInsert", 2, map_get_or_insert);
    r.method(mp, "getOrInsertComputed", 2, map_get_or_insert_computed);
    r.method(mp, "keys", 0, map_keys);
    r.method(mp, "values", 0, map_values);
    let entries = r.method(mp, "entries", 0, map_entries);
    mp.borrow_mut().props.insert(PropKey::Sym(r.sym_iterator.clone()), Property::hidden(Value::Object(entries)));
    r.getter(mp, "size", map_size);
    r.tag(mp, "Map");

    let sp = &r.set_proto;
    let sc = r.ctor("Set", 0, set_ctor, sp);
    r.getter_sym(&sc, &r.sym_species, "[Symbol.species]", super::array::return_this);
    let add = r.method(sp, "add", 1, set_add);
    super::set_native_data(&sc, Value::Object(add));
    r.method(sp, "has", 1, set_has);
    r.method(sp, "delete", 1, set_delete);
    r.method(sp, "clear", 0, set_clear);
    r.method(sp, "forEach", 1, set_for_each);
    r.method(sp, "entries", 0, set_entries);
    let values = r.method(sp, "values", 0, set_values);
    sp.borrow_mut().props.insert(PropKey::str("keys"), Property::hidden(Value::Object(values.clone())));
    sp.borrow_mut().props.insert(PropKey::Sym(r.sym_iterator.clone()), Property::hidden(Value::Object(values)));
    r.getter(sp, "size", set_size);
    r.method(sp, "union", 1, set_union);
    r.method(sp, "intersection", 1, set_intersection);
    r.method(sp, "difference", 1, set_difference);
    r.method(sp, "symmetricDifference", 1, set_symmetric_difference);
    r.method(sp, "isSubsetOf", 1, set_is_subset_of);
    r.method(sp, "isSupersetOf", 1, set_is_superset_of);
    r.method(sp, "isDisjointFrom", 1, set_is_disjoint_from);
    r.tag(sp, "Set");

    for ip in [&r.map_iter_proto, &r.set_iter_proto] {
        r.method(ip, "next", 0, iter_next);
    }
    r.tag(&r.map_iter_proto, "Map Iterator");
    r.tag(&r.set_iter_proto, "Set Iterator");

    let wm = &r.weakmap_proto;
    let wmc = r.ctor("WeakMap", 0, weakmap_ctor, wm);
    r.method(wm, "get", 1, weak_get);
    let set = r.method(wm, "set", 2, weakmap_set);
    super::set_native_data(&wmc, Value::Object(set));
    r.method(wm, "has", 1, weakmap_has);
    r.method(wm, "delete", 1, weakmap_delete);
    r.method(wm, "getOrInsert", 2, weak_get_or_insert);
    r.method(wm, "getOrInsertComputed", 2, weak_get_or_insert_computed);
    r.tag(wm, "WeakMap");

    let ws = &r.weakset_proto;
    let wsc = r.ctor("WeakSet", 0, weakset_ctor, ws);
    let add = r.method(ws, "add", 1, weakset_add);
    super::set_native_data(&wsc, Value::Object(add));
    r.method(ws, "has", 1, weakset_has);
    r.method(ws, "delete", 1, weakset_delete);
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

/// Preenche a coleção a partir de um iterável (AddEntriesFromIterable e o
/// laço do construtor de Set), chamando `adder`. Se `adder` ainda for o
/// método original (guardado no construtor), grava direto.
fn fill_from(vm: &mut Vm, a: &CallArgs, o: &ObjRef, adder_name: &str, pairs: bool) -> Result<(), Value> {
    let iterable = a.arg(0);
    if iterable.is_nullish() {
        return Ok(());
    }
    let ov = Value::Object(o.clone());
    let adder = vm.get_str(&ov, adder_name)?;
    if !adder.is_callable() {
        let d = vm.describe(&adder);
        return Err(vm.type_error(format!("'{d}' returned for property '{adder_name}' of object '{}' is not a function", vm.describe(&ov))));
    }
    let original = matches!((&adder, super::native_data(&a.callee)), (Value::Object(x), Value::Object(y)) if x.ptr_eq(&y));
    let rec = vm.get_iterator(&iterable)?;
    // Set com o add original sobre um array comum: nada roda no meio, copia direto
    if original && !pairs {
        let it = match &rec.borrow().kind {
            Kind::IterRecord { iter: Value::Object(i), next: Value::Object(n), .. } if n.ptr_eq(&vm.realm.array_iter_next) => Some(i.clone()),
            _ => None,
        };
        if let Some(rest) = it.and_then(|i| super::array::array_iter_rest(&i)) {
            for k in rest {
                add_direct(vm, o, k, Value::Undefined, false)?;
            }
            return Ok(());
        }
    }
    while let Some(item) = vm.iter_step(&rec)? {
        let r = (|| -> Result<(), Value> {
            let (k, v) = if pairs {
                if !matches!(item, Value::Object(_)) {
                    let d = vm.describe(&item);
                    return Err(vm.type_error(format!("Iterator value {d} is not an entry object")));
                }
                (vm.get_index(&item, &Value::Number(0.0))?, vm.get_index(&item, &Value::Number(1.0))?)
            } else {
                (item.clone(), Value::Undefined)
            };
            if original {
                return add_direct(vm, o, k, v, pairs);
            }
            let args = if pairs { vec![k, v] } else { vec![k] };
            vm.call(&adder, ov.clone(), args).map(|_| ())
        })();
        if let Err(e) = r {
            let _ = vm.iter_close(&rec);
            return Err(e);
        }
    }
    Ok(())
}

/// O que Map.prototype.set / Set.prototype.add / WeakMap.prototype.set /
/// WeakSet.prototype.add fariam.
fn add_direct(vm: &mut Vm, o: &ObjRef, k: Value, v: Value, pairs: bool) -> Result<(), Value> {
    let weak_ok = can_be_weak(vm, &k);
    let mut b = o.borrow_mut();
    match &mut b.kind {
        Kind::Map(c) => c.set(k, v),
        Kind::Set(c) => c.set(k.clone(), k),
        Kind::WeakMap(c) | Kind::WeakSet(c) => {
            if !weak_ok {
                drop(b);
                let d = vm.describe(&k);
                let msg = if pairs { format!("Invalid value used as weak map key: {d}") } else { format!("Invalid value used in weak set: {d}") };
                return Err(vm.type_error(msg));
            }
            c.set(&k, if pairs { v } else { Value::Bool(true) });
        }
        _ => {}
    }
    Ok(())
}

fn map_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "Map")?;
    let def = vm.realm.map_proto.clone();
    let o = new_coll(vm, &nt, &def, false)?;
    fill_from(vm, a, &o, "set", true)?;
    Ok(Value::Object(o))
}

fn set_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "Set")?;
    let def = vm.realm.set_proto.clone();
    let o = new_coll(vm, &nt, &def, true)?;
    fill_from(vm, a, &o, "add", false)?;
    Ok(Value::Object(o))
}

fn weakmap_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "WeakMap")?;
    let def = vm.realm.weakmap_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let o = ObjRef::new(Object::new(Some(proto), Kind::WeakMap(WeakColl::default())));
    fill_from(vm, a, &o, "set", true)?;
    Ok(Value::Object(o))
}

fn weakset_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "WeakSet")?;
    let def = vm.realm.weakset_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let o = ObjRef::new(Object::new(Some(proto), Kind::WeakSet(WeakColl::default())));
    fill_from(vm, a, &o, "add", false)?;
    Ok(Value::Object(o))
}

fn incompatible(vm: &mut Vm, a: &CallArgs, what: &str) -> Value {
    let d = vm.describe(&a.this);
    vm.type_error(format!("Method {what} called on incompatible receiver {d}"))
}

/// Acessa a coleção fraca de `this` (WeakSet se `set`, senão WeakMap).
fn with_weak<R>(vm: &mut Vm, a: &CallArgs, set: bool, what: &str, f: impl FnOnce(&mut WeakColl) -> R) -> Result<R, Value> {
    if let Value::Object(o) = &a.this {
        let mut b = o.borrow_mut();
        match (&mut b.kind, set) {
            (Kind::WeakMap(c), false) | (Kind::WeakSet(c), true) => return Ok(f(c)),
            _ => {}
        }
    }
    Err(incompatible(vm, a, what))
}

fn weak_get(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(with_weak(vm, a, false, "WeakMap.prototype.get", |c| c.get(&k))?.unwrap_or(Value::Undefined))
}

fn weakmap_has(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_weak(vm, a, false, "WeakMap.prototype.has", |c| c.has(&k))?))
}

fn weakset_has(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_weak(vm, a, true, "WeakSet.prototype.has", |c| c.has(&k))?))
}

fn weakmap_delete(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_weak(vm, a, false, "WeakMap.prototype.delete", |c| c.delete(&k))?))
}

fn weakset_delete(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_weak(vm, a, true, "WeakSet.prototype.delete", |c| c.delete(&k))?))
}

/// Acessa a coleção de `this` (Set se `set`, senão Map).
fn with_coll<R>(vm: &mut Vm, a: &CallArgs, set: bool, what: &str, f: impl FnOnce(&mut Collection) -> R) -> Result<R, Value> {
    if let Value::Object(o) = &a.this {
        let mut b = o.borrow_mut();
        match (&mut b.kind, set) {
            (Kind::Map(c), false) | (Kind::Set(c), true) => return Ok(f(c)),
            _ => {}
        }
    }
    Err(incompatible(vm, a, what))
}

fn map_get(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(with_coll(vm, a, false, "Map.prototype.get", |c| c.get(&k))?.unwrap_or(Value::Undefined))
}

fn map_set(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (k, v) = (a.arg(0), a.arg(1));
    with_coll(vm, a, false, "Map.prototype.set", |c| c.set(k, v))?;
    Ok(a.this.clone())
}

fn map_get_or_insert(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (k, v) = (a.arg(0), a.arg(1));
    with_coll(vm, a, false, "Map.prototype.getOrInsert", |c| match c.get(&k) {
        Some(old) => old,
        None => {
            c.set(k, v.clone());
            v
        }
    })
}

fn map_get_or_insert_computed(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    const WHAT: &str = "Map.prototype.getOrInsertComputed";
    with_coll(vm, a, false, WHAT, |_| ())?;
    let f = a.arg(1);
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    let k = canonical(a.arg(0));
    if let Some(old) = with_coll(vm, a, false, WHAT, |c| c.get(&k))? {
        return Ok(old);
    }
    let v = vm.call(&f, Value::Undefined, vec![k.clone()])?;
    with_coll(vm, a, false, WHAT, |c| c.set(k, v.clone()))?;
    Ok(v)
}

/// CanBeHeldWeakly: objeto ou símbolo que não veio de Symbol.for.
fn can_be_weak(vm: &Vm, v: &Value) -> bool {
    match v {
        Value::Object(_) => true,
        Value::Symbol(s) => !s.description().is_some_and(|d| vm.symbol_registry.get(d).is_some_and(|r| r == s)),
        _ => false,
    }
}

fn weak_key_arg(vm: &mut Vm, k: &Value) -> Result<(), Value> {
    if can_be_weak(vm, k) {
        return Ok(());
    }
    let d = vm.describe(k);
    Err(vm.type_error(format!("Invalid value used as weak map key: {d}")))
}

fn weakmap_set(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    with_weak(vm, a, false, "WeakMap.prototype.set", |_| ())?;
    let (k, v) = (a.arg(0), a.arg(1));
    weak_key_arg(vm, &k)?;
    with_weak(vm, a, false, "WeakMap.prototype.set", |c| c.set(&k, v))?;
    Ok(a.this.clone())
}

fn weak_get_or_insert(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    const WHAT: &str = "WeakMap.prototype.getOrInsert";
    with_weak(vm, a, false, WHAT, |_| ())?;
    let (k, v) = (a.arg(0), a.arg(1));
    weak_key_arg(vm, &k)?;
    with_weak(vm, a, false, WHAT, |c| match c.get(&k) {
        Some(old) => old,
        None => {
            c.set(&k, v.clone());
            v
        }
    })
}

fn weak_get_or_insert_computed(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    const WHAT: &str = "WeakMap.prototype.getOrInsertComputed";
    with_weak(vm, a, false, WHAT, |_| ())?;
    let f = a.arg(1);
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    let k = a.arg(0);
    weak_key_arg(vm, &k)?;
    if let Some(old) = with_weak(vm, a, false, WHAT, |c| c.get(&k))? {
        return Ok(old);
    }
    let v = vm.call(&f, Value::Undefined, vec![k.clone()])?;
    with_weak(vm, a, false, WHAT, |c| c.set(&k, v.clone()))?;
    Ok(v)
}

fn set_add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    with_coll(vm, a, true, "Set.prototype.add", |c| {
        let key = k.clone();
        c.set(key, k)
    })?;
    Ok(a.this.clone())
}

fn weakset_add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    with_weak(vm, a, true, "WeakSet.prototype.add", |_| ())?;
    if !can_be_weak(vm, &a.arg(0)) {
        let d = vm.describe(&a.arg(0));
        return Err(vm.type_error(format!("Invalid value used in weak set: {d}")));
    }
    let k = a.arg(0);
    with_weak(vm, a, true, "WeakSet.prototype.add", |c| c.set(&k, Value::Bool(true)))?;
    Ok(a.this.clone())
}

fn map_has(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_coll(vm, a, false, "Map.prototype.has", |c| c.has(&k))?))
}

fn set_has(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_coll(vm, a, true, "Set.prototype.has", |c| c.has(&k))?))
}

fn map_delete(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_coll(vm, a, false, "Map.prototype.delete", |c| c.delete(&k))?))
}

fn set_delete(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_coll(vm, a, true, "Set.prototype.delete", |c| c.delete(&k))?))
}

fn map_clear(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    with_coll(vm, a, false, "Map.prototype.clear", |c| c.clear())?;
    Ok(Value::Undefined)
}

fn set_clear(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    with_coll(vm, a, true, "Set.prototype.clear", |c| c.clear())?;
    Ok(Value::Undefined)
}

fn map_size(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Number(with_coll(vm, a, false, "get Map.prototype.size", |c| c.size())? as f64))
}

fn set_size(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Number(with_coll(vm, a, true, "get Set.prototype.size", |c| c.size())? as f64))
}

fn map_for_each(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    coll_for_each(vm, a, false, "Map.prototype.forEach")
}

fn set_for_each(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    coll_for_each(vm, a, true, "Set.prototype.forEach")
}

fn coll_for_each(vm: &mut Vm, a: &CallArgs, is_set: bool, what: &str) -> Result<Value, Value> {
    with_coll(vm, a, is_set, what, |_| ())?;
    let f = a.arg(0);
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    let mut i = 0;
    loop {
        // relê a cada passo: o callback pode alterar a coleção
        let entry = with_coll(vm, a, is_set, what, |c| {
            while i < c.entries.len() {
                if let Some(e) = &c.entries[i] {
                    i += 1;
                    return Some(e.clone());
                }
                i += 1;
            }
            None
        })?;
        let Some((k, v)) = entry else { break };
        let args = if is_set { vec![k.clone(), k, a.this.clone()] } else { vec![v, k, a.this.clone()] };
        vm.call(&f, a.arg(1), args)?;
    }
    Ok(Value::Undefined)
}

fn make_iter(vm: &mut Vm, a: &CallArgs, is_set: bool, what: &str, kind: IterKind) -> Result<Value, Value> {
    with_coll(vm, a, is_set, what, |_| ())?;
    let Value::Object(o) = &a.this else { unreachable!() };
    let proto = if is_set { vm.realm.set_iter_proto.clone() } else { vm.realm.map_iter_proto.clone() };
    Ok(Value::Object(ObjRef::new(Object::new(
        Some(proto),
        Kind::MapIter {
            target: o.clone(),
            index: 0,
            kind,
        },
    ))))
}

fn map_keys(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, false, "Map.prototype.keys", IterKind::Keys)
}

fn map_values(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, false, "Map.prototype.values", IterKind::Values)
}

fn map_entries(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, false, "Map.prototype.entries", IterKind::Entries)
}

fn set_values(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, true, "Set.prototype.values", IterKind::Values)
}

fn set_entries(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, true, "Set.prototype.entries", IterKind::Entries)
}

fn iter_next(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(it) = &a.this else {
        return Err(vm.type_error("next method called on incompatible receiver"));
    };
    let (target, index, kind) = match &it.borrow().kind {
        Kind::MapIter { target, index, kind } => (target.clone(), *index, *kind),
        _ => return Err(vm.type_error("next method called on incompatible receiver")),
    };
    let found = {
        let b = target.borrow();
        let c = match &b.kind {
            Kind::Map(c) | Kind::Set(c) => c,
            _ => return Ok(iter_result(vm, Value::Undefined, true)),
        };
        let mut i = index;
        let mut out = None;
        while i < c.entries.len() {
            if let Some(e) = &c.entries[i] {
                out = Some((i, e.clone()));
                break;
            }
            i += 1;
        }
        out
    };
    match found {
        Some((i, (k, v))) => {
            if let Kind::MapIter { index, .. } = &mut it.borrow_mut().kind {
                *index = i + 1;
            }
            let val = match kind {
                IterKind::Keys => k,
                IterKind::Values => v,
                IterKind::Entries => vm.array_value(vec![k, v]),
            };
            Ok(iter_result(vm, val, false))
        }
        None => {
            if let Kind::MapIter { index, .. } = &mut it.borrow_mut().kind {
                *index = usize::MAX / 2;
            }
            Ok(iter_result(vm, Value::Undefined, true))
        }
    }
}

fn map_group_by(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let items = a.arg(0);
    if items.is_nullish() {
        let d = vm.describe(&items);
        return Err(vm.type_error(format!("{d} is not iterable")));
    }
    let f = a.arg(1);
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    let mut c = Collection::default();
    let rec = vm.get_iterator(&items)?;
    let mut i = 0usize;
    while let Some(item) = vm.iter_step(&rec)? {
        let k = match vm.call(&f, Value::Undefined, vec![item.clone(), Value::Number(i as f64)]) {
            Ok(k) => canonical(k),
            Err(e) => {
                let _ = vm.iter_close(&rec);
                return Err(e);
            }
        };
        i += 1;
        match c.get(&k) {
            Some(Value::Object(arr)) => {
                if let Kind::Array(v) = &mut arr.borrow_mut().kind {
                    v.push(item);
                }
            }
            _ => {
                let arr = vm.array_value(vec![item]);
                c.set(k, arr);
            }
        }
    }
    Ok(Value::Object(ObjRef::new(Object::new(Some(vm.realm.map_proto.clone()), Kind::Map(c)))))
}

// ------------------------------------------------- métodos de conjunto

/// Set Record do spec (GetSetRecord): o objeto "parecido com Set".
struct SetRec {
    obj: Value,
    size: f64,
    has: Value,
    keys: Value,
}

fn get_set_record(vm: &mut Vm, v: &Value, what: &str) -> Result<SetRec, Value> {
    if !matches!(v, Value::Object(_)) {
        return Err(vm.type_error(format!("{what} argument must be an object")));
    }
    let raw = vm.get_str(v, "size")?;
    let n = vm.to_number(&raw)?;
    if n.is_nan() {
        return Err(vm.type_error("The .size property is NaN"));
    }
    let size = crate::conv::to_integer(n);
    if size < 0.0 {
        return Err(vm.range_error(format!("'{}' is an invalid size", crate::conv::number_to_string(size))));
    }
    let get_fn = |vm: &mut Vm, name: &str| -> Result<Value, Value> {
        let f = vm.get_str(v, name)?;
        if !f.is_callable() {
            let d = vm.describe(&f);
            return Err(vm.type_error(format!("{d} is not a function")));
        }
        Ok(f)
    };
    let has = get_fn(vm, "has")?;
    let keys = get_fn(vm, "keys")?;
    Ok(SetRec { obj: v.clone(), size, has, keys })
}

impl SetRec {
    /// GetIteratorFromMethod(obj, keys).
    fn keys_iter(&self, vm: &mut Vm) -> Result<super::iterator::IterRec, Value> {
        let it = vm.call(&self.keys, self.obj.clone(), Vec::new())?;
        if !matches!(it, Value::Object(_)) {
            return Err(vm.type_error("Result of the keys method is not an object"));
        }
        super::iterator::get_iterator_direct(vm, &it)
    }

    fn has(&self, vm: &mut Vm, v: &Value) -> Result<bool, Value> {
        Ok(crate::vm::truthy(&vm.call(&self.has, self.obj.clone(), vec![v.clone()])?))
    }
}

/// `this` dos métodos de conjunto e o Set Record do argumento.
fn set_op_args(vm: &mut Vm, a: &CallArgs, name: &str) -> Result<(ObjRef, SetRec), Value> {
    let what = format!("Set.prototype.{name}");
    with_coll(vm, a, true, &what, |_| ())?;
    let Value::Object(o) = &a.this else { unreachable!() };
    let rec = get_set_record(vm, &a.arg(0), &what)?;
    Ok((o.clone(), rec))
}

fn set_data<R>(o: &ObjRef, f: impl FnOnce(&Collection) -> R) -> R {
    match &o.borrow().kind {
        Kind::Set(c) => f(c),
        _ => f(&Collection::default()),
    }
}

/// Elemento `i` da lista do Set (None = buraco) e o tamanho atual da lista.
fn set_entry(o: &ObjRef, i: usize) -> (Option<Value>, usize) {
    set_data(o, |c| (c.entries.get(i).cloned().flatten().map(|e| e.0), c.entries.len()))
}

fn set_copy(o: &ObjRef) -> Collection {
    let mut out = Collection::default();
    set_data(o, |c| {
        for (k, _) in c.entries.iter().flatten() {
            out.set(k.clone(), k.clone());
        }
    });
    out
}

fn new_set(vm: &Vm, c: Collection) -> Value {
    Value::Object(ObjRef::new(Object::new(Some(vm.realm.set_proto.clone()), Kind::Set(c))))
}

/// CanonicalizeKeyedCollectionKey: -0 vira +0.
fn canonical(v: Value) -> Value {
    match v {
        Value::Number(n) if n == 0.0 => Value::Number(0.0),
        v => v,
    }
}

/// Percorre os elementos do próprio Set chamando `has` do outro, relendo o
/// tamanho a cada passo; para quando `stop(inOther)` devolve true.
fn scan_this(vm: &mut Vm, o: &ObjRef, other: &SetRec, mut each: impl FnMut(&Value, bool) -> bool) -> Result<bool, Value> {
    let mut i = 0;
    let mut len = set_entry(o, 0).1;
    while i < len {
        let (e, _) = set_entry(o, i);
        i += 1;
        if let Some(e) = e {
            let in_other = other.has(vm, &e)?;
            if each(&e, in_other) {
                return Ok(true);
            }
            len = set_entry(o, 0).1;
        }
    }
    Ok(false)
}

fn set_union(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, other) = set_op_args(vm, a, "union")?;
    let mut keys = other.keys_iter(vm)?;
    let mut out = set_copy(&o);
    while let Some(v) = keys.step_value(vm)? {
        let v = canonical(v);
        if !out.has(&v) {
            out.set(v.clone(), v);
        }
    }
    Ok(new_set(vm, out))
}

fn set_intersection(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, other) = set_op_args(vm, a, "intersection")?;
    let mut out = Collection::default();
    if set_data(&o, |c| c.size()) as f64 <= other.size {
        scan_this(vm, &o, &other, |e, in_other| {
            if in_other && !out.has(e) {
                out.set(e.clone(), e.clone());
            }
            false
        })?;
    } else {
        let mut keys = other.keys_iter(vm)?;
        while let Some(v) = keys.step_value(vm)? {
            let v = canonical(v);
            if set_data(&o, |c| c.has(&v)) && !out.has(&v) {
                out.set(v.clone(), v);
            }
        }
    }
    Ok(new_set(vm, out))
}

fn set_difference(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, other) = set_op_args(vm, a, "difference")?;
    let mut out = set_copy(&o);
    if set_data(&o, |c| c.size()) as f64 <= other.size {
        // percorre a cópia (fixa): mudanças no próprio Set não contam
        let items: Vec<Value> = out.entries.iter().flatten().map(|e| e.0.clone()).collect();
        for e in items {
            if other.has(vm, &e)? {
                out.delete(&e);
            }
        }
    } else {
        let mut keys = other.keys_iter(vm)?;
        while let Some(v) = keys.step_value(vm)? {
            out.delete(&canonical(v));
        }
    }
    Ok(new_set(vm, out))
}

fn set_symmetric_difference(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, other) = set_op_args(vm, a, "symmetricDifference")?;
    let mut keys = other.keys_iter(vm)?;
    let mut out = set_copy(&o);
    while let Some(v) = keys.step_value(vm)? {
        let v = canonical(v);
        let already = out.has(&v);
        if set_data(&o, |c| c.has(&v)) {
            if already {
                out.delete(&v);
            }
        } else if !already {
            out.set(v.clone(), v);
        }
    }
    Ok(new_set(vm, out))
}

fn set_is_subset_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, other) = set_op_args(vm, a, "isSubsetOf")?;
    if set_data(&o, |c| c.size()) as f64 > other.size {
        return Ok(Value::Bool(false));
    }
    let missing = scan_this(vm, &o, &other, |_, in_other| !in_other)?;
    Ok(Value::Bool(!missing))
}

/// Algum valor de `keys()` do outro está (ou não está, se `want` = false) no próprio Set?
fn scan_other(vm: &mut Vm, o: &ObjRef, other: &SetRec, want: bool) -> Result<bool, Value> {
    let mut keys = other.keys_iter(vm)?;
    while let Some(v) = keys.step_value(vm)? {
        if set_data(o, |c| c.has(&v)) == want {
            super::iterator::close_normal(vm, &keys.iter)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_is_superset_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, other) = set_op_args(vm, a, "isSupersetOf")?;
    if (set_data(&o, |c| c.size()) as f64) < other.size {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(!scan_other(vm, &o, &other, false)?))
}

fn set_is_disjoint_from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, other) = set_op_args(vm, a, "isDisjointFrom")?;
    let common = if set_data(&o, |c| c.size()) as f64 <= other.size {
        scan_this(vm, &o, &other, |_, in_other| in_other)?
    } else {
        scan_other(vm, &o, &other, true)?
    };
    Ok(Value::Bool(!common))
}

/// Protótipo padrão de WeakRef/FinalizationRegistry: o `prototype` do construtor.
fn own_proto(vm: &Vm, a: &CallArgs) -> ObjRef {
    match crate::ops::data_in_chain(&a.callee, "prototype") {
        Some(Value::Object(p)) => p,
        _ => vm.realm.object_proto.clone(),
    }
}

fn weakref_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "WeakRef")?;
    if !can_be_weak(vm, &a.arg(0)) {
        return Err(vm.type_error("WeakRef: target must be an object"));
    }
    let def = own_proto(vm, a);
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let Some((_, target)) = WeakKey::of(&a.arg(0)) else { return Err(vm.type_error("WeakRef: target must be an object")) };
    Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::WeakRef(target)))))
}

fn weakref_deref(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(o) = &a.this
        && let Kind::WeakRef(t) = &o.borrow().kind
    {
        return Ok(t.get().unwrap_or(Value::Undefined));
    }
    Err(vm.type_error("WeakRef.prototype.deref called on incompatible receiver"))
}

fn finreg_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "FinalizationRegistry")?;
    let def = own_proto(vm, a);
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::Ordinary))))
}
