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

