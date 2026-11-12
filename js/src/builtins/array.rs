//! Array, Array.prototype e o iterador de arrays.

use std::cell::RefCell;
use std::collections::HashSet;

use super::{Realm, rel_index, this_obj};
use crate::ops::index_key;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Vm, same_value_zero, strict_equals, truthy};

pub fn install(r: &Realm) {
    let proto = &r.array_proto;
    let c = r.ctor("Array", 1, array_ctor, proto);
    r.method(&c, "isArray", 1, is_array);
    r.method(&c, "of", 0, of);
    r.method(&c, "from", 1, from);
    r.getter_sym(&c, &r.sym_species, "[Symbol.species]", species);

    r.method(proto, "at", 1, at);
    r.method(proto, "concat", 1, concat);
    r.method(proto, "copyWithin", 2, copy_within);
    r.method(proto, "entries", 0, entries);
    r.method(proto, "every", 1, every);
    r.method(proto, "fill", 1, fill);
    r.method(proto, "filter", 1, filter);
    r.method(proto, "find", 1, find);
    r.method(proto, "findIndex", 1, find_index);
    r.method(proto, "findLast", 1, find_last);
    r.method(proto, "findLastIndex", 1, find_last_index);
    r.method(proto, "flat", 0, flat);
    r.method(proto, "flatMap", 1, flat_map);
    r.method(proto, "forEach", 1, for_each);
    r.method(proto, "includes", 1, includes);
    r.method(proto, "indexOf", 1, index_of);
    r.method(proto, "join", 1, join);
    r.method(proto, "keys", 0, keys);
    r.method(proto, "lastIndexOf", 1, last_index_of);
    r.method(proto, "map", 1, map);
    r.method(proto, "pop", 0, pop);
    r.method(proto, "push", 1, push);
    r.method(proto, "reduce", 1, reduce);
    r.method(proto, "reduceRight", 1, reduce_right);
    r.method(proto, "reverse", 0, reverse);
    r.method(proto, "shift", 0, shift);
    r.method(proto, "slice", 2, slice);
    r.method(proto, "some", 1, some);
    r.method(proto, "sort", 1, sort);
    r.method(proto, "splice", 2, splice);
    r.method(proto, "toLocaleString", 0, to_locale_string);
    r.method(proto, "toReversed", 0, to_reversed);
    r.method(proto, "toSorted", 1, to_sorted);
    r.method(proto, "toSpliced", 2, to_spliced);
    r.method(proto, "toString", 0, to_string);
    r.method(proto, "unshift", 1, unshift);
    r.method(proto, "with", 2, with);
    let values = r.array_values.clone();
    proto.borrow_mut().props.insert(PropKey::str("values"), Property::hidden(Value::Object(values.clone())));
    proto.borrow_mut().props.insert(PropKey::Sym(r.sym_iterator.clone()), Property::hidden(Value::Object(values)));

    let ip = &r.array_iter_proto;
    ip.borrow_mut().props.insert(PropKey::str("next"), Property::hidden(Value::Object(r.array_iter_next.clone())));
    r.tag(ip, "Array Iterator");
    // %IteratorPrototype%[Symbol.iterator] devolve o próprio iterador
    r.method_sym(&r.iterator_proto, &r.sym_iterator, "[Symbol.iterator]", 0, return_this);
}

pub fn return_this(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(a.this.clone())
}

fn species(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(a.this.clone())
}

/// Objeto `{ value, done }` dos iteradores.
pub fn iter_result(vm: &mut Vm, value: Value, done: bool) -> Value {
    let o = vm.new_object();
    {
        let mut b = o.borrow_mut();
        b.props.insert(PropKey::str("value"), Property::data(value));
        b.props.insert(PropKey::str("done"), Property::data(Value::Bool(done)));
    }
    Value::Object(o)
}

// ------------------------------------------------------ auxiliares

fn get_i(vm: &mut Vm, o: &ObjRef, i: usize) -> Result<Value, Value> {
    if let Kind::Array(v) = &o.borrow().kind
        && let Some(x) = v.get(i)
        && !matches!(x, Value::Empty)
    {
        return Ok(x.clone());
    }
    vm.get_obj(o, &index_key(i))
}

fn has_i(vm: &mut Vm, o: &ObjRef, i: usize) -> bool {
    if let Kind::Array(v) = &o.borrow().kind
        && let Some(x) = v.get(i)
        && !matches!(x, Value::Empty)
    {
        return true;
    }
    vm.has_property(o, &index_key(i)).unwrap_or(false)
}

fn set_i(vm: &mut Vm, o: &ObjRef, i: usize, v: Value) -> Result<(), Value> {
    vm.put(&Value::Object(o.clone()), index_key(i), v, true)
}

fn set_len(vm: &mut Vm, o: &ObjRef, n: usize) -> Result<(), Value> {
    vm.put(&Value::Object(o.clone()), PropKey::str("length"), Value::Number(n as f64), true)
}

fn del_i(vm: &mut Vm, o: &ObjRef, i: usize) -> Result<(), Value> {
    vm.delete_value(&Value::Object(o.clone()), &index_key(i), true)?;
    Ok(())
}

/// Array comum, mutável: dá para mexer direto no vetor.
fn is_plain_array(o: &ObjRef) -> bool {
    let b = o.borrow();
    matches!(b.kind, Kind::Array(_)) && !b.frozen && b.extensible
}

fn callback(vm: &mut Vm, a: &CallArgs, what: &str) -> Result<Value, Value> {
    let f = a.arg(0);
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function (Array.prototype.{what})")));
    }
    Ok(f)
}

// ------------------------------------------------------ construtor

fn array_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let def = vm.realm.array_proto.clone();
    let proto = vm.proto_from_ctor(a.new_target.as_ref(), &def)?;
    let items = if a.args.len() == 1 {
        match &a.args[0] {
            Value::Number(n) => {
                let len = *n as u32;
                if len as f64 != *n {
                    return Err(vm.range_error("Invalid array length"));
                }
                if len > 50_000_000 {
                    return Err(vm.range_error("Invalid array length"));
                }
                vec![Value::Empty; len as usize]
            }
            v => vec![v.clone()],
        }
    } else {
        a.args.clone()
    };
    Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::Array(items)))))
}

fn is_array(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(vm.is_array(&a.arg(0))?))
}

fn of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(vm.array_value(a.args.clone()))
}

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let src = a.arg(0);
    let map = a.arg(1);
    let this_arg = a.arg(2);
    if !map.is_undefined() && !map.is_callable() {
        return Err(vm.type_error("Array.from: when provided, the second argument must be a function"));
    }
    if src.is_nullish() {
        return Err(vm.type_error(format!("{} is not iterable", vm.describe(&src))));
    }
    let key = PropKey::Sym(vm.realm.sym_iterator.clone());
    let iter = vm.get(&src, &key)?;
    let mut items = if !iter.is_nullish() {
        vm.iterate_to_vec(&src)?
    } else {
        let o = vm.to_object(&src)?;
        let len = vm.length_of(&o)?;
        let mut v = Vec::with_capacity(len.min(1 << 16));
        for i in 0..len {
            v.push(get_i(vm, &o, i)?);
        }
        v
    };
    if map.is_callable() {
        for (i, item) in items.iter_mut().enumerate() {
            *item = vm.call(&map, this_arg.clone(), vec![item.clone(), Value::Number(i as f64)])?;
        }
    }
    // subclasses: new this() quando this é um construtor diferente de Array
    if let Value::Object(c) = &a.this
        && c.is_constructor()
        && !matches!(vm.realm.global.borrow().props.get(&PropKey::str("Array")), Some(Property { slot: Slot::Data { value: Value::Object(x), .. }, .. }) if x.ptr_eq(c))
    {
        let obj = vm.construct(c, Vec::new(), c)?;
        if let Value::Object(o) = &obj {
            let n = items.len();
            for (i, v) in items.into_iter().enumerate() {
                vm.define_data(o, index_key(i), v);
            }
            set_len(vm, o, n)?;
        }
        return Ok(obj);
    }
    Ok(vm.array_value(items))
}

// ------------------------------------------------------ métodos

