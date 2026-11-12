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

