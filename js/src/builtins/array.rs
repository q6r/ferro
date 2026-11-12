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

