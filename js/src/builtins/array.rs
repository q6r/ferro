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

fn at(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let len = vm.length_of(&o)? as f64;
    let n = vm.to_integer(&a.arg(0))?;
    let k = if n >= 0.0 { n } else { len + n };
    if k < 0.0 || k >= len {
        return Ok(Value::Undefined);
    }
    get_i(vm, &o, k as usize)
}

fn is_spreadable(vm: &mut Vm, v: &Value) -> Result<bool, Value> {
    let Value::Object(o) = v else { return Ok(false) };
    let key = PropKey::Sym(vm.realm.sym_is_concat_spreadable.clone());
    let s = vm.get_obj(o, &key)?;
    if !s.is_undefined() {
        return Ok(truthy(&s));
    }
    Ok(matches!(o.borrow().kind, Kind::Array(_)))
}

fn concat(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let mut out = Vec::new();
    let mut items = vec![Value::Object(o)];
    items.extend(a.args.iter().cloned());
    for item in items {
        if is_spreadable(vm, &item)? {
            let Value::Object(e) = &item else { continue };
            if let Kind::Array(v) = &e.borrow().kind {
                out.extend(v.iter().cloned());
                continue;
            }
            let len = vm.length_of(e)?;
            for i in 0..len {
                if has_i(vm, e, i) {
                    out.push(get_i(vm, e, i)?);
                } else {
                    out.push(Value::Empty);
                }
            }
        } else {
            out.push(item);
        }
    }
    Ok(vm.array_value(out))
}

fn copy_within(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let len = vm.length_of(&o)?;
    let to = rel_index(vm, &a.arg(0), len, 0)?;
    let from = rel_index(vm, &a.arg(1), len, 0)?;
    let end = rel_index(vm, &a.arg(2), len, len)?;
    let count = end.saturating_sub(from).min(len - to);
    let vals: Vec<(bool, Value)> = (0..count)
        .map(|k| {
            let h = has_i(vm, &o, from + k);
            let v = if h { get_i(vm, &o, from + k) } else { Ok(Value::Undefined) };
            v.map(|v| (h, v))
        })
        .collect::<Result<_, _>>()?;
    for (k, (h, v)) in vals.into_iter().enumerate() {
        if h {
            set_i(vm, &o, to + k, v)?;
        } else {
            del_i(vm, &o, to + k)?;
        }
    }
    Ok(Value::Object(o))
}

fn make_iter(vm: &mut Vm, a: &CallArgs, kind: IterKind) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let it = ObjRef::new(Object::new(
        Some(vm.realm.array_iter_proto.clone()),
        Kind::ArrayIter {
            target: Value::Object(o),
            index: 0,
            kind,
        },
    ));
    Ok(Value::Object(it))
}

fn entries(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, IterKind::Entries)
}

fn keys(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, IterKind::Keys)
}

pub fn values(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, IterKind::Values)
}

pub fn array_iter_next(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(it) = &a.this else {
        return Err(vm.type_error("next method called on incompatible receiver"));
    };
    if let Some(r) = vm.array_iter_fast(it) {
        return Ok(match r {
            Some(v) => iter_result(vm, v, false),
            None => iter_result(vm, Value::Undefined, true),
        });
    }
    let (target, index, kind) = match &it.borrow().kind {
        Kind::ArrayIter { target, index, kind } => (target.clone(), *index, *kind),
        _ => return Err(vm.type_error("next method called on incompatible receiver")),
    };
    let Value::Object(t) = &target else {
        return Ok(iter_result(vm, Value::Undefined, true));
    };
    let len = if index == usize::MAX { 0 } else { vm.length_of(t)? };
    if index >= len {
        if let Kind::ArrayIter { index, .. } = &mut it.borrow_mut().kind {
            *index = usize::MAX;
        }
        return Ok(iter_result(vm, Value::Undefined, true));
    }
    if let Kind::ArrayIter { index, .. } = &mut it.borrow_mut().kind {
        *index += 1;
    }
    let v = match kind {
        IterKind::Keys => Value::Number(index as f64),
        IterKind::Values => get_i(vm, t, index)?,
        IterKind::Entries => {
            let v = get_i(vm, t, index)?;
            vm.array_value(vec![Value::Number(index as f64), v])
        }
    };
    Ok(iter_result(vm, v, false))
}

fn every(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let f = callback(vm, a, "every")?;
    let len = vm.length_of(&o)?;
    for i in 0..len {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            let r = vm.call(&f, a.arg(1), vec![v, Value::Number(i as f64), Value::Object(o.clone())])?;
            if !truthy(&r) {
                return Ok(Value::Bool(false));
            }
        }
    }
    Ok(Value::Bool(true))
}

fn some(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let f = callback(vm, a, "some")?;
    let len = vm.length_of(&o)?;
    for i in 0..len {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            let r = vm.call(&f, a.arg(1), vec![v, Value::Number(i as f64), Value::Object(o.clone())])?;
            if truthy(&r) {
                return Ok(Value::Bool(true));
            }
        }
    }
    Ok(Value::Bool(false))
}

fn fill(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let len = vm.length_of(&o)?;
    let start = rel_index(vm, &a.arg(1), len, 0)?;
    let end = rel_index(vm, &a.arg(2), len, len)?;
    let v = a.arg(0);
    if is_plain_array(&o) {
        if let Kind::Array(vec) = &mut o.borrow_mut().kind {
            for x in vec.iter_mut().take(end).skip(start) {
                *x = v.clone();
            }
        }
        return Ok(Value::Object(o));
    }
    for i in start..end {
        set_i(vm, &o, i, v.clone())?;
    }
    Ok(Value::Object(o))
}

fn filter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let f = callback(vm, a, "filter")?;
    let len = vm.length_of(&o)?;
    let mut out = Vec::new();
    for i in 0..len {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            let r = vm.call(&f, a.arg(1), vec![v.clone(), Value::Number(i as f64), Value::Object(o.clone())])?;
            if truthy(&r) {
                out.push(v);
            }
        }
    }
    Ok(vm.array_value(out))
}

fn find_generic(vm: &mut Vm, a: &CallArgs, rev: bool, want_index: bool, what: &str) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let f = callback(vm, a, what)?;
    let len = vm.length_of(&o)?;
    for k in 0..len {
        let i = if rev { len - 1 - k } else { k };
        let v = get_i(vm, &o, i)?;
        let r = vm.call(&f, a.arg(1), vec![v.clone(), Value::Number(i as f64), Value::Object(o.clone())])?;
        if truthy(&r) {
            return Ok(if want_index { Value::Number(i as f64) } else { v });
        }
    }
    Ok(if want_index { Value::Number(-1.0) } else { Value::Undefined })
}

fn find(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, false, false, "find")
}

fn find_index(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, false, true, "findIndex")
}

fn find_last(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, true, false, "findLast")
}

fn find_last_index(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, true, true, "findLastIndex")
}

fn flatten_into(vm: &mut Vm, out: &mut Vec<Value>, src: &ObjRef, depth: f64) -> Result<(), Value> {
    let len = vm.length_of(src)?;
    for i in 0..len {
        if !has_i(vm, src, i) {
            continue;
        }
        let v = get_i(vm, src, i)?;
        if depth > 0.0
            && let Value::Object(inner) = &v
            && matches!(inner.borrow().kind, Kind::Array(_))
        {
            flatten_into(vm, out, inner, depth - 1.0)?;
            continue;
        }
        out.push(v);
    }
    Ok(())
}

fn flat(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let depth = if a.arg(0).is_undefined() { 1.0 } else { vm.to_integer(&a.arg(0))? };
    let mut out = Vec::new();
    flatten_into(vm, &mut out, &o, depth)?;
    Ok(vm.array_value(out))
}

fn flat_map(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let f = callback(vm, a, "flatMap")?;
    let len = vm.length_of(&o)?;
    let mut out = Vec::new();
    for i in 0..len {
        if !has_i(vm, &o, i) {
            continue;
        }
        let v = get_i(vm, &o, i)?;
        let r = vm.call(&f, a.arg(1), vec![v, Value::Number(i as f64), Value::Object(o.clone())])?;
        match &r {
            Value::Object(inner) if matches!(inner.borrow().kind, Kind::Array(_)) => {
                flatten_into(vm, &mut out, inner, 0.0)?;
            }
            _ => out.push(r),
        }
    }
    Ok(vm.array_value(out))
}

fn for_each(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let f = callback(vm, a, "forEach")?;
    let len = vm.length_of(&o)?;
    let this_arg = a.arg(1);
    for i in 0..len {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            vm.call(&f, this_arg.clone(), vec![v, Value::Number(i as f64), Value::Object(o.clone())])?;
        }
    }
    Ok(Value::Undefined)
}

fn includes(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let len = vm.length_of(&o)?;
    let start = rel_index(vm, &a.arg(1), len, 0)?;
    let target = a.arg(0);
    if let Kind::Array(v) = &o.borrow().kind {
        return Ok(Value::Bool(v.iter().skip(start).any(|x| {
            let x = if matches!(x, Value::Empty) { &Value::Undefined } else { x };
            same_value_zero(x, &target)
        })));
    }
    for i in start..len {
        let v = get_i(vm, &o, i)?;
        if same_value_zero(&v, &target) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn index_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let len = vm.length_of(&o)?;
    let start = rel_index(vm, &a.arg(1), len, 0)?;
    let target = a.arg(0);
    if let Kind::Array(v) = &o.borrow().kind {
        let pos = v.iter().enumerate().skip(start).find(|(_, x)| strict_equals(x, &target) && !matches!(x, Value::Empty));
        return Ok(Value::Number(pos.map_or(-1.0, |(i, _)| i as f64)));
    }
    for i in start..len {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            if strict_equals(&v, &target) {
                return Ok(Value::Number(i as f64));
            }
        }
    }
    Ok(Value::Number(-1.0))
}

fn last_index_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let len = vm.length_of(&o)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let start = if a.args.len() > 1 {
        let n = vm.to_integer(&a.arg(1))?;
        if n < 0.0 {
            let k = len as f64 + n;
            if k < 0.0 {
                return Ok(Value::Number(-1.0));
            }
            k as usize
        } else {
            (n as usize).min(len - 1)
        }
    } else {
        len - 1
    };
    let target = a.arg(0);
    for i in (0..=start).rev() {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            if strict_equals(&v, &target) {
                return Ok(Value::Number(i as f64));
            }
        }
    }
    Ok(Value::Number(-1.0))
}

thread_local! {
    /// Arrays sendo convertidos em texto agora (evita recursão infinita).
    static JOINING: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
}

pub fn join_values(vm: &mut Vm, o: &ObjRef, sep: &JsStr) -> Result<JsStr, Value> {
    let id = o.id();
    if JOINING.with(|j| j.borrow().contains(&id)) {
        return Ok(JsStr::empty());
    }
    JOINING.with(|j| j.borrow_mut().insert(id));
    let result = (|| {
        let len = vm.length_of(o)?;
        let mut out: Vec<u16> = Vec::new();
        for i in 0..len {
            if i > 0 {
                out.extend_from_slice(sep.units());
            }
            let v = get_i(vm, o, i)?;
            if !v.is_nullish() {
                let s = vm.to_string(&v)?;
                out.extend_from_slice(s.units());
            }
        }
        Ok(JsStr::from_units(out))
    })();
    JOINING.with(|j| j.borrow_mut().remove(&id));
    result
}

fn join(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let sep = if a.arg(0).is_undefined() { JsStr::new(",") } else { vm.to_string(&a.arg(0))? };
    Ok(Value::String(join_values(vm, &o, &sep)?))
}

fn map(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let f = callback(vm, a, "map")?;
    let len = vm.length_of(&o)?;
    let this_arg = a.arg(1);
    let mut out = Vec::with_capacity(len.min(1 << 20));
    for i in 0..len {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            out.push(vm.call(&f, this_arg.clone(), vec![v, Value::Number(i as f64), Value::Object(o.clone())])?);
        } else {
            out.push(Value::Empty);
        }
    }
    Ok(vm.array_value(out))
}

fn pop(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    if is_plain_array(&o)
        && let Kind::Array(v) = &mut o.borrow_mut().kind
    {
        return Ok(match v.pop() {
            Some(Value::Empty) | None => Value::Undefined,
            Some(x) => x,
        });
    }
    let len = vm.length_of(&o)?;
    if len == 0 {
        set_len(vm, &o, 0)?;
        return Ok(Value::Undefined);
    }
    let v = get_i(vm, &o, len - 1)?;
    del_i(vm, &o, len - 1)?;
    set_len(vm, &o, len - 1)?;
    Ok(v)
}

fn push(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    if is_plain_array(&o)
        && let Kind::Array(v) = &mut o.borrow_mut().kind
    {
        v.extend(a.args.iter().cloned());
        return Ok(Value::Number(v.len() as f64));
    }
    let len = vm.length_of(&o)?;
    for (k, v) in a.args.iter().enumerate() {
        set_i(vm, &o, len + k, v.clone())?;
    }
    let n = len + a.args.len();
    set_len(vm, &o, n)?;
    Ok(Value::Number(n as f64))
}

fn reduce_generic(vm: &mut Vm, a: &CallArgs, right: bool) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    let f = callback(vm, a, if right { "reduceRight" } else { "reduce" })?;
    let len = vm.length_of(&o)?;
    let order: Vec<usize> = if right { (0..len).rev().collect() } else { (0..len).collect() };
    let mut it = order.into_iter();
    let mut acc = if a.args.len() >= 2 {
        a.arg(1)
    } else {
        loop {
            match it.next() {
                Some(i) => {
                    if has_i(vm, &o, i) {
                        break get_i(vm, &o, i)?;
                    }
                }
                None => return Err(vm.type_error("Reduce of empty array with no initial value")),
            }
        }
    };
    for i in it {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            acc = vm.call(&f, Value::Undefined, vec![acc, v, Value::Number(i as f64), Value::Object(o.clone())])?;
        }
    }
    Ok(acc)
}

fn reduce(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    reduce_generic(vm, a, false)
}

fn reduce_right(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    reduce_generic(vm, a, true)
}

fn reverse(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    if is_plain_array(&o) {
        if let Kind::Array(v) = &mut o.borrow_mut().kind {
            v.reverse();
        }
        return Ok(Value::Object(o));
    }
    let len = vm.length_of(&o)?;
    let mut lo = 0;
    while lo < len / 2 {
        let hi = len - 1 - lo;
        let (hl, hh) = (has_i(vm, &o, lo), has_i(vm, &o, hi));
        let vl = get_i(vm, &o, lo)?;
        let vh = get_i(vm, &o, hi)?;
        if hh {
            set_i(vm, &o, lo, vh)?;
        } else {
            del_i(vm, &o, lo)?;
        }
        if hl {
            set_i(vm, &o, hi, vl)?;
        } else {
            del_i(vm, &o, hi)?;
        }
        lo += 1;
    }
    Ok(Value::Object(o))
}

fn shift(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    if is_plain_array(&o)
        && let Kind::Array(v) = &mut o.borrow_mut().kind
    {
        if v.is_empty() {
            return Ok(Value::Undefined);
        }
        return Ok(match v.remove(0) {
            Value::Empty => Value::Undefined,
            x => x,
        });
    }
    let len = vm.length_of(&o)?;
    if len == 0 {
        set_len(vm, &o, 0)?;
        return Ok(Value::Undefined);
    }
    let first = get_i(vm, &o, 0)?;
    for i in 1..len {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            set_i(vm, &o, i - 1, v)?;
        } else {
            del_i(vm, &o, i - 1)?;
        }
    }
    del_i(vm, &o, len - 1)?;
    set_len(vm, &o, len - 1)?;
    Ok(first)
}

fn unshift(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_obj(vm, a)?;
    if is_plain_array(&o)
        && let Kind::Array(v) = &mut o.borrow_mut().kind
    {
        let tail = std::mem::take(v);
        v.extend(a.args.iter().cloned());
        v.extend(tail);
        return Ok(Value::Number(v.len() as f64));
    }
    let len = vm.length_of(&o)?;
    let n = a.args.len();
    for i in (0..len).rev() {
        if has_i(vm, &o, i) {
            let v = get_i(vm, &o, i)?;
            set_i(vm, &o, i + n, v)?;
        } else {
            del_i(vm, &o, i + n)?;
        }
    }
    for (k, v) in a.args.iter().enumerate() {
        set_i(vm, &o, k, v.clone())?;
    }
    set_len(vm, &o, len + n)?;
    Ok(Value::Number((len + n) as f64))
}

