//! ArrayBuffer, typed arrays (Uint8Array, Float64Array...) e DataView.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use super::{Realm, native_data, native_fn, rel_index, set_native_data};
use crate::bigint::BigInt;
use crate::value::*;
use crate::vm::{Vm, same_value_zero, strict_equals, truthy};

pub fn install(r: &Realm) {
    // ArrayBuffer
    let abp = &r.array_buffer_proto;
    let ab = r.ctor("ArrayBuffer", 1, ab_ctor, abp);
    r.method(&ab, "isView", 1, is_view);
    r.getter_sym(&ab, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.getter(abp, "byteLength", ab_byte_length);
    r.getter(abp, "maxByteLength", ab_byte_length);
    r.getter(abp, "resizable", false_getter);
    r.getter(abp, "detached", false_getter);
    r.method(abp, "slice", 2, ab_slice);
    r.tag(abp, "ArrayBuffer");

    // %TypedArray% (abstrato)
    let tap = &r.typed_array_proto;
    let ta = native_fn(&r.function_proto, "TypedArray", 0, abstract_ctor, true);
    ta.borrow_mut().props.insert(
        PropKey::str("prototype"),
        Property {
            slot: Slot::Data {
                value: Value::Object(tap.clone()),
                writable: false,
            },
            enumerable: false,
            configurable: false,
        },
    );
    tap.borrow_mut().props.insert(PropKey::str("constructor"), Property::hidden(Value::Object(ta.clone())));
    r.method(&ta, "from", 1, ta_from);
    r.method(&ta, "of", 0, ta_of);
    r.getter_sym(&ta, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.getter(tap, "buffer", get_buffer);
    r.getter(tap, "byteLength", get_byte_length);
    r.getter(tap, "byteOffset", get_byte_offset);
    r.getter(tap, "length", get_length);
    r.getter_sym(tap, &r.sym_to_string_tag, "[Symbol.toStringTag]", get_tag);
    let methods: &[(&str, u32, NativeFn)] = &[
        ("at", 1, at),
        ("copyWithin", 2, copy_within),
        ("every", 1, every),
        ("fill", 1, fill),
        ("filter", 1, filter),
        ("find", 1, find),
        ("findIndex", 1, find_index),
        ("findLast", 1, find_last),
        ("findLastIndex", 1, find_last_index),
        ("forEach", 1, for_each),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("join", 1, join),
        ("lastIndexOf", 1, last_index_of),
        ("map", 1, map),
        ("reduce", 1, reduce),
        ("reduceRight", 1, reduce_right),
        ("reverse", 0, reverse),
        ("set", 1, set),
        ("slice", 2, slice),
        ("some", 1, some),
        ("sort", 1, sort),
        ("subarray", 2, subarray),
        ("toLocaleString", 0, to_locale_string),
        ("toReversed", 0, to_reversed),
        ("toSorted", 1, to_sorted),
        ("with", 2, with),
    ];
    for (name, len, f) in methods {
        r.method(tap, name, *len, *f);
    }
    // entries/keys/values usam o iterador de arrays
    for name in ["entries", "keys", "values"] {
        if let Some(p) = r.array_proto.borrow().props.get(&PropKey::str(name)).cloned() {
            tap.borrow_mut().props.insert(PropKey::str(name), p.clone());
            if name == "values" {
                tap.borrow_mut().props.insert(PropKey::Sym(r.sym_iterator.clone()), p);
            }
        }
    }
    if let Some(p) = r.array_proto.borrow().props.get(&PropKey::str("toString")).cloned() {
        tap.borrow_mut().props.insert(PropKey::str("toString"), p);
    }

    for (i, kind) in TaKind::ALL.iter().enumerate() {
        let proto = &r.typed_protos[i];
        let c = r.ctor(kind.name(), 3, ta_ctor, proto);
        c.borrow_mut().proto = Some(ta.clone());
        set_native_data(&c, Value::Number(i as f64));
        let bpe = Value::Number(kind.size() as f64);
        r.constant(&c, "BYTES_PER_ELEMENT", bpe.clone());
        r.constant(proto, "BYTES_PER_ELEMENT", bpe);
    }

    // DataView
    let dvp = &r.data_view_proto;
    r.ctor("DataView", 1, dv_ctor, dvp);
    r.getter(dvp, "buffer", get_buffer);
    r.getter(dvp, "byteLength", dv_byte_length);
    r.getter(dvp, "byteOffset", get_byte_offset);
    r.tag(dvp, "DataView");
    let dv: &[(&str, &str, NativeFn, NativeFn)] = &[
        ("getInt8", "setInt8", dv_get_i8, dv_set_i8),
        ("getUint8", "setUint8", dv_get_u8, dv_set_u8),
        ("getInt16", "setInt16", dv_get_i16, dv_set_i16),
        ("getUint16", "setUint16", dv_get_u16, dv_set_u16),
        ("getInt32", "setInt32", dv_get_i32, dv_set_i32),
        ("getUint32", "setUint32", dv_get_u32, dv_set_u32),
        ("getFloat32", "setFloat32", dv_get_f32, dv_set_f32),
        ("getFloat64", "setFloat64", dv_get_f64, dv_set_f64),
        ("getBigInt64", "setBigInt64", dv_get_bi64, dv_set_bi64),
        ("getBigUint64", "setBigUint64", dv_get_bu64, dv_set_bu64),
    ];
    for (g, s, gf, sf) in dv {
        r.method(dvp, g, 1, *gf);
        r.method(dvp, s, 2, *sf);
    }
}

fn false_getter(_vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(false))
}

fn abstract_ctor(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Err(vm.type_error("Abstract class TypedArray not directly constructable"))
}

/// ToIndex (tamanhos e posições).
fn to_index(vm: &mut Vm, v: &Value, what: &str) -> Result<usize, Value> {
    if v.is_undefined() {
        return Ok(0);
    }
    let n = vm.to_integer(v)?;
    if !(0.0..=9007199254740991.0).contains(&n) {
        if what == "typed array length" {
            let d = crate::conv::number_to_string(n);
            return Err(vm.range_error(format!("Invalid {what}: {d}")));
        }
        return Err(vm.range_error(format!("Invalid {what}")));
    }
    Ok(n as usize)
}

pub fn new_buffer(vm: &mut Vm, len: usize) -> Result<ObjRef, Value> {
    if len > 2_000_000_000 {
        return Err(vm.range_error("Array buffer allocation failed"));
    }
    let data: Bytes = Rc::new(RefCell::new(vec![0u8; len]));
    Ok(ObjRef::new(Object::new(Some(vm.realm.array_buffer_proto.clone()), Kind::ArrayBuffer(data))))
}

fn buffer_bytes(o: &ObjRef) -> Option<Bytes> {
    match &o.borrow().kind {
        Kind::ArrayBuffer(b) => Some(b.clone()),
        _ => None,
    }
}

fn ab_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return Err(vm.type_error("Constructor ArrayBuffer requires 'new'"));
    };
    let len = to_index(vm, &a.arg(0), "array buffer length")?;
    let b = new_buffer(vm, len)?;
    let def = vm.realm.array_buffer_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    b.borrow_mut().proto = Some(proto);
    Ok(Value::Object(b))
}

fn is_view(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(matches!(&a.arg(0), Value::Object(o) if matches!(o.borrow().kind, Kind::TypedArray(_) | Kind::DataView(_)))))
}

fn this_buffer(vm: &mut Vm, a: &CallArgs) -> Result<(ObjRef, Bytes), Value> {
    if let Value::Object(o) = &a.this
        && let Some(b) = buffer_bytes(o)
    {
        return Ok((o.clone(), b));
    }
    Err(vm.type_error("Method ArrayBuffer.prototype called on incompatible receiver"))
}

fn ab_byte_length(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, b) = this_buffer(vm, a)?;
    let n = b.borrow().len();
    Ok(Value::Number(n as f64))
}

fn ab_slice(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, b) = this_buffer(vm, a)?;
    let len = b.borrow().len();
    let start = rel_index(vm, &a.arg(0), len, 0)?;
    let end = rel_index(vm, &a.arg(1), len, len)?;
    let part: Vec<u8> = if start < end { b.borrow()[start..end].to_vec() } else { Vec::new() };
    let nb = new_buffer(vm, 0)?;
    if let Kind::ArrayBuffer(d) = &nb.borrow().kind {
        *d.borrow_mut() = part;
    }
    Ok(Value::Object(nb))
}

// ------------------------------------------------------------ typed arrays

fn kind_of_ctor(f: &ObjRef) -> TaKind {
    match native_data(f) {
        Value::Number(n) => TaKind::ALL[n as usize],
        _ => TaKind::U8,
    }
}

/// Cria um typed array novo (com buffer próprio zerado).
pub fn new_typed(vm: &mut Vm, kind: TaKind, length: usize, proto: Option<ObjRef>) -> Result<ObjRef, Value> {
    let Some(bytes) = length.checked_mul(kind.size()) else {
        return Err(vm.range_error(format!("Invalid typed array length: {length}")));
    };
    let buf = new_buffer(vm, bytes)?;
    let data = buffer_bytes(&buf).unwrap();
    let idx = TaKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
    let proto = proto.unwrap_or_else(|| vm.realm.typed_protos[idx].clone());
    Ok(ObjRef::new(Object::new(
        Some(proto),
        Kind::TypedArray(Box::new(TypedArrayData {
            data,
            buffer: buf,
            kind,
            offset: 0,
            length,
        })),
    )))
}

fn view(o: &ObjRef, buffer: ObjRef, kind: TaKind, offset: usize, length: usize, proto: ObjRef) -> ObjRef {
    let data = buffer_bytes(&buffer).unwrap_or_default();
    let _ = o;
    ObjRef::new(Object::new(
        Some(proto),
        Kind::TypedArray(Box::new(TypedArrayData {
            data,
            buffer,
            kind,
            offset,
            length,
        })),
    ))
}

fn ta_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let kind = kind_of_ctor(&a.callee);
    let Some(nt) = a.new_target.clone() else {
        return Err(vm.type_error(format!("Constructor {} requires 'new'", kind.name())));
    };
    let idx = TaKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
    let def = vm.realm.typed_protos[idx].clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let first = a.arg(0);
    let Value::Object(src) = &first else {
        let len = to_index(vm, &first, "typed array length")?;
        return Ok(Value::Object(new_typed(vm, kind, len, Some(proto))?));
    };
    // sobre um ArrayBuffer existente
    if let Some(bytes) = buffer_bytes(src) {
        let blen = bytes.borrow().len();
        let size = kind.size();
        let offset = to_index(vm, &a.arg(1), "typed array offset")?;
        if offset % size != 0 {
            return Err(vm.range_error(format!("start offset of {} should be a multiple of {size}", kind.name())));
        }
        let length = if a.arg(2).is_undefined() {
            if blen % size != 0 {
                return Err(vm.range_error(format!("byte length of {} should be a multiple of {size}", kind.name())));
            }
            if offset > blen {
                return Err(vm.range_error(format!("Start offset {offset} is outside the bounds of the buffer")));
            }
            (blen - offset) / size
        } else {
            let l = to_index(vm, &a.arg(2), "typed array length")?;
            if offset + l * size > blen {
                return Err(vm.range_error(format!("Invalid typed array length: {l}")));
            }
            l
        };
        return Ok(Value::Object(view(src, src.clone(), kind, offset, length, proto)));
    }
    // cópia de outro typed array, de um iterável ou de um array-like
    let from_typed = match &src.borrow().kind {
        Kind::TypedArray(t) => Some((t.kind.is_bigint(), (0..t.length).filter_map(|i| t.get(i)).collect::<Vec<Value>>())),
        _ => None,
    };
    let values = if let Some((big, vals)) = from_typed {
        if big != kind.is_bigint() {
            return Err(vm.type_error("Cannot mix BigInt and other types, use explicit conversions"));
        }
        vals
    } else {
        let key = PropKey::Sym(vm.realm.sym_iterator.clone());
        let it = vm.get_obj(src, &key)?;
        if it.is_callable() {
            vm.iterate_to_vec(&first)?
        } else {
            let len = vm.length_of(src)?;
            let mut v = Vec::with_capacity(len.min(1 << 20));
            for i in 0..len {
                v.push(vm.get_obj(src, &crate::ops::index_key(i))?);
            }
            v
        }
    };
    let t = new_typed(vm, kind, values.len(), Some(proto))?;
    for (i, v) in values.iter().enumerate() {
        vm.typed_set(&t, i, v)?;
    }
    Ok(Value::Object(t))
}

/// (objeto, tipo, tamanho) de `this`, que precisa ser um typed array.
fn this_ta(vm: &mut Vm, a: &CallArgs) -> Result<(ObjRef, TaKind, usize), Value> {
    if let Value::Object(o) = &a.this
        && let Kind::TypedArray(t) = &o.borrow().kind
    {
        return Ok((o.clone(), t.kind, t.length));
    }
    let d = vm.describe(&a.this);
    Err(vm.type_error(format!("this is not a typed array ({d})")))
}

fn elem(o: &ObjRef, i: usize) -> Value {
    match &o.borrow().kind {
        Kind::TypedArray(t) => t.get(i).unwrap_or_default(),
        _ => Value::Undefined,
    }
}

fn get_buffer(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(o) = &a.this
        && let Kind::TypedArray(t) | Kind::DataView(t) = &o.borrow().kind
    {
        return Ok(Value::Object(t.buffer.clone()));
    }
    Err(vm.type_error("not a typed array or DataView"))
}

fn get_byte_length(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, kind, len) = this_ta(vm, a)?;
    Ok(Value::Number((len * kind.size()) as f64))
}

fn get_byte_offset(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(o) = &a.this
        && let Kind::TypedArray(t) | Kind::DataView(t) = &o.borrow().kind
    {
        return Ok(Value::Number(t.offset as f64));
    }
    Err(vm.type_error("not a typed array or DataView"))
}

fn get_length(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, _, len) = this_ta(vm, a)?;
    Ok(Value::Number(len as f64))
}

fn get_tag(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(o) = &a.this
        && let Kind::TypedArray(t) = &o.borrow().kind
    {
        return Ok(Value::str(t.kind.name()));
    }
    Ok(Value::Undefined)
}

fn ta_from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(c) = a.this.clone() else {
        return Err(vm.type_error("TypedArray.from: this is not a constructor"));
    };
    let src = a.arg(0);
    let map = a.arg(1);
    let key = PropKey::Sym(vm.realm.sym_iterator.clone());
    let it = vm.get(&src, &key)?;
    let mut vals = if it.is_callable() {
        vm.iterate_to_vec(&src)?
    } else {
        let o = vm.to_object(&src)?;
        let len = vm.length_of(&o)?;
        let mut v = Vec::new();
        for i in 0..len {
            v.push(vm.get_obj(&o, &crate::ops::index_key(i))?);
        }
        v
    };
    if map.is_callable() {
        for (i, v) in vals.iter_mut().enumerate() {
            *v = vm.call(&map, a.arg(2), vec![v.clone(), Value::Number(i as f64)])?;
        }
    }
    let arr = vm.array_value(vals);
    vm.construct(&c, vec![arr], &c)
}

fn ta_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(c) = a.this.clone() else {
        return Err(vm.type_error("TypedArray.of: this is not a constructor"));
    };
    let arr = vm.array_value(a.args.clone());
    vm.construct(&c, vec![arr], &c)
}

fn callback(vm: &mut Vm, a: &CallArgs) -> Result<Value, Value> {
    let f = a.arg(0);
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    Ok(f)
}

fn at(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let n = vm.to_integer(&a.arg(0))?;
    let k = if n >= 0.0 { n } else { len as f64 + n };
    if k < 0.0 || k >= len as f64 {
        return Ok(Value::Undefined);
    }
    Ok(elem(&o, k as usize))
}

fn copy_within(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a)?;
    let to = rel_index(vm, &a.arg(0), len, 0)?;
    let from = rel_index(vm, &a.arg(1), len, 0)?;
    let end = rel_index(vm, &a.arg(2), len, len)?;
    let count = end.saturating_sub(from).min(len - to);
    if let Kind::TypedArray(t) = &o.borrow().kind {
        let sz = kind.size();
        let mut d = t.data.borrow_mut();
        let base = t.offset;
        d.copy_within(base + from * sz..base + (from + count) * sz, base + to * sz);
    }
    Ok(a.this.clone())
}

fn iterate_cb(vm: &mut Vm, a: &CallArgs, mut f: impl FnMut(&mut Vm, usize, Value, Value) -> Result<bool, Value>) -> Result<(), Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let cb = callback(vm, a)?;
    for i in 0..len {
        let v = elem(&o, i);
        let r = vm.call(&cb, a.arg(1), vec![v.clone(), Value::Number(i as f64), a.this.clone()])?;
        if !f(vm, i, v, r)? {
            break;
        }
    }
    Ok(())
}

fn every(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut ok = true;
    iterate_cb(vm, a, |_, _, _, r| {
        if !truthy(&r) {
            ok = false;
        }
        Ok(ok)
    })?;
    Ok(Value::Bool(ok))
}

fn some(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut found = false;
    iterate_cb(vm, a, |_, _, _, r| {
        if truthy(&r) {
            found = true;
        }
        Ok(!found)
    })?;
    Ok(Value::Bool(found))
}

fn for_each(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    iterate_cb(vm, a, |_, _, _, _| Ok(true))?;
    Ok(Value::Undefined)
}

fn find_generic(vm: &mut Vm, a: &CallArgs, rev: bool, want_index: bool) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let cb = callback(vm, a)?;
    for k in 0..len {
        let i = if rev { len - 1 - k } else { k };
        let v = elem(&o, i);
        let r = vm.call(&cb, a.arg(1), vec![v.clone(), Value::Number(i as f64), a.this.clone()])?;
        if truthy(&r) {
            return Ok(if want_index { Value::Number(i as f64) } else { v });
        }
    }
    Ok(if want_index { Value::Number(-1.0) } else { Value::Undefined })
}

fn find(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, false, false)
}
fn find_index(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, false, true)
}
fn find_last(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, true, false)
}
fn find_last_index(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, true, true)
}

fn fill(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a)?;
    let v = if kind.is_bigint() {
        Value::BigInt(super::bigint::to_bigint(vm, &a.arg(0))?)
    } else {
        Value::Number(vm.to_number(&a.arg(0))?)
    };
    let start = rel_index(vm, &a.arg(1), len, 0)?;
    let end = rel_index(vm, &a.arg(2), len, len)?;
    for i in start..end.max(start) {
        vm.typed_set(&o, i, &v)?;
    }
    Ok(a.this.clone())
}

fn from_values(vm: &mut Vm, kind: TaKind, vals: &[Value]) -> Result<Value, Value> {
    let t = new_typed(vm, kind, vals.len(), None)?;
    for (i, v) in vals.iter().enumerate() {
        vm.typed_set(&t, i, v)?;
    }
    Ok(Value::Object(t))
}

fn filter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, kind, _) = this_ta(vm, a)?;
    let mut kept = Vec::new();
    iterate_cb(vm, a, |_, _, v, r| {
        if truthy(&r) {
            kept.push(v);
        }
        Ok(true)
    })?;
    from_values(vm, kind, &kept)
}

fn map(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, kind, _) = this_ta(vm, a)?;
    let mut out = Vec::new();
    iterate_cb(vm, a, |_, _, _, r| {
        out.push(r);
        Ok(true)
    })?;
    from_values(vm, kind, &out)
}

fn includes(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let start = rel_index(vm, &a.arg(1), len, 0)?;
    let target = a.arg(0);
    Ok(Value::Bool((start..len).any(|i| same_value_zero(&elem(&o, i), &target))))
}

fn index_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let start = rel_index(vm, &a.arg(1), len, 0)?;
    let target = a.arg(0);
    Ok(Value::Number((start..len).find(|&i| strict_equals(&elem(&o, i), &target)).map_or(-1.0, |i| i as f64)))
}

fn last_index_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
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
    Ok(Value::Number((0..=start).rev().find(|&i| strict_equals(&elem(&o, i), &target)).map_or(-1.0, |i| i as f64)))
}

fn join(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let sep = if a.arg(0).is_undefined() { ",".to_string() } else { vm.to_rust_string(&a.arg(0))? };
    let mut parts = Vec::with_capacity(len);
    for i in 0..len {
        parts.push(vm.to_rust_string(&elem(&o, i))?);
    }
    Ok(Value::str(&parts.join(&sep)))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let mut parts = Vec::with_capacity(len);
    for i in 0..len {
        let v = elem(&o, i);
        let f = vm.get_str(&v, "toLocaleString")?;
        let s = vm.call(&f, v, Vec::new())?;
        parts.push(vm.to_rust_string(&s)?);
    }
    Ok(Value::str(&parts.join(",")))
}

fn reduce_generic(vm: &mut Vm, a: &CallArgs, right: bool) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let cb = callback(vm, a)?;
    let order: Vec<usize> = if right { (0..len).rev().collect() } else { (0..len).collect() };
    let mut it = order.into_iter();
    let mut acc = if a.args.len() >= 2 {
        a.arg(1)
    } else {
        match it.next() {
            Some(i) => elem(&o, i),
            None => return Err(vm.type_error("Reduce of empty array with no initial value")),
        }
    };
    for i in it {
        acc = vm.call(&cb, Value::Undefined, vec![acc, elem(&o, i), Value::Number(i as f64), a.this.clone()])?;
    }
    Ok(acc)
}

fn reduce(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    reduce_generic(vm, a, false)
}

fn reduce_right(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    reduce_generic(vm, a, true)
}

fn values_of(o: &ObjRef, len: usize) -> Vec<Value> {
    (0..len).map(|i| elem(o, i)).collect()
}

fn reverse(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let vals = values_of(&o, len);
    for (i, v) in vals.into_iter().rev().enumerate() {
        vm.typed_set(&o, i, &v)?;
    }
    Ok(a.this.clone())
}

fn set(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let offset = vm.to_integer(&a.arg(1))?;
    if offset < 0.0 {
        return Err(vm.range_error("offset is out of bounds"));
    }
    let offset = offset as usize;
    let src = a.arg(0);
    let vals = match &src {
        Value::Object(s) if matches!(s.borrow().kind, Kind::TypedArray(_)) => {
            let n = vm.length_of(s)?;
            values_of(s, n)
        }
        _ => {
            let s = vm.to_object(&src)?;
            let n = vm.length_of(&s)?;
            let mut v = Vec::with_capacity(n);
            for i in 0..n {
                v.push(vm.get_obj(&s, &crate::ops::index_key(i))?);
            }
            v
        }
    };
    if offset + vals.len() > len {
        return Err(vm.range_error("offset is out of bounds"));
    }
    for (i, v) in vals.iter().enumerate() {
        vm.typed_set(&o, offset + i, v)?;
    }
    Ok(Value::Undefined)
}

fn slice(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a)?;
    let start = rel_index(vm, &a.arg(0), len, 0)?;
    let end = rel_index(vm, &a.arg(1), len, len)?;
    let vals: Vec<Value> = (start..end.max(start)).map(|i| elem(&o, i)).collect();
    from_values(vm, kind, &vals)
}

fn subarray(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a)?;
    let start = rel_index(vm, &a.arg(0), len, 0)?;
    let end = rel_index(vm, &a.arg(1), len, len)?;
    let (buffer, base) = match &o.borrow().kind {
        Kind::TypedArray(t) => (t.buffer.clone(), t.offset),
        _ => return Ok(Value::Undefined),
    };
    let proto = o.proto().unwrap_or_else(|| vm.realm.typed_array_proto.clone());
    Ok(Value::Object(view(&o, buffer, kind, base + start * kind.size(), end.saturating_sub(start), proto)))
}

/// Ordem padrão dos typed arrays: numérica, NaN no fim, -0 antes de +0.
fn numeric_order(x: &Value, y: &Value) -> Ordering {
    match (x, y) {
        (Value::Number(a), Value::Number(b)) => {
            if a.is_nan() {
                return if b.is_nan() { Ordering::Equal } else { Ordering::Greater };
            }
            if b.is_nan() {
                return Ordering::Less;
            }
            match a.partial_cmp(b) {
                Some(Ordering::Equal) => a.is_sign_positive().cmp(&b.is_sign_positive()),
                Some(o) => o,
                None => Ordering::Equal,
            }
        }
        (Value::BigInt(a), Value::BigInt(b)) => a.cmp(b),
        _ => Ordering::Equal,
    }
}

fn sorted_values(vm: &mut Vm, o: &ObjRef, len: usize, cmp: &Value) -> Result<Vec<Value>, Value> {
    let mut vals = values_of(o, len);
    if cmp.is_undefined() {
        vals.sort_by(numeric_order);
        return Ok(vals);
    }
    if !cmp.is_callable() {
        return Err(vm.type_error("The comparison function must be either a function or undefined"));
    }
    super::array::sort_values(vm, vals, cmp)
}

fn sort(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a)?;
    let vals = sorted_values(vm, &o, len, &a.arg(0))?;
    for (i, v) in vals.iter().enumerate() {
        vm.typed_set(&o, i, v)?;
    }
    Ok(a.this.clone())
}

fn to_sorted(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a)?;
    let vals = sorted_values(vm, &o, len, &a.arg(0))?;
    from_values(vm, kind, &vals)
}

fn to_reversed(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a)?;
    let mut vals = values_of(&o, len);
    vals.reverse();
    from_values(vm, kind, &vals)
}

fn with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a)?;
    let n = vm.to_integer(&a.arg(0))?;
    let idx = if n >= 0.0 { n } else { len as f64 + n };
    if idx < 0.0 || idx >= len as f64 {
        return Err(vm.range_error("Invalid typed array index"));
    }
    let mut vals = values_of(&o, len);
    vals[idx as usize] = a.arg(1);
    from_values(vm, kind, &vals)
}

// ---------------------------------------------------------------- DataView

