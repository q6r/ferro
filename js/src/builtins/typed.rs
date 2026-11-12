//! ArrayBuffer, typed arrays (Uint8Array, Float64Array...) e DataView.
//!
//! Um buffer desanexado (`transfer`) fica com o vetor vazio: as vistas
//! antigas passam a enxergar tamanho 0 sem precisar consultar o buffer a
//! cada acesso. Buffers redimensionáveis mudam o tamanho do vetor no lugar;
//! as vistas criadas sem tamanho explícito (`tracking`) acompanham.

use std::cmp::Ordering;
use std::rc::Rc;

use super::{Realm, native_data, native_fn, rel_index, set_native_data};
use crate::ops::index_key;
use crate::value::*;
use crate::vm::{Vm, same_value_zero, strict_equals, truthy};

/// Maior ArrayBuffer que aceitamos alocar.
const MAX_BYTES: usize = 2_000_000_000;

pub fn install(r: &Realm) {
    // ArrayBuffer
    let abp = &r.array_buffer_proto;
    let ab = r.ctor("ArrayBuffer", 1, ab_ctor, abp);
    *r.array_buffer_ctor.borrow_mut() = Some(ab.clone());
    r.method(&ab, "isView", 1, is_view);
    r.getter_sym(&ab, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.getter(abp, "byteLength", ab_byte_length);
    r.getter(abp, "maxByteLength", ab_max_byte_length);
    r.getter(abp, "resizable", ab_resizable);
    r.getter(abp, "detached", ab_detached);
    r.method(abp, "slice", 2, ab_slice);
    r.method(abp, "resize", 1, ab_resize);
    r.method(abp, "transfer", 0, ab_transfer);
    r.method(abp, "transferToFixedLength", 0, ab_transfer_fixed);
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
        ("entries", 0, entries),
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
        ("keys", 0, keys),
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
    // [Symbol.iterator] é a mesma função que values
    let values = r.method(tap, "values", 0, values);
    tap.borrow_mut().props.insert(PropKey::Sym(r.sym_iterator.clone()), Property::hidden(Value::Object(values)));
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
        r.typed_ctors.borrow_mut().push(c);
    }
    let u8_ctor = r.typed_ctors.borrow()[TaKind::U8.index()].clone();
    super::base64::install(r, &u8_ctor, &r.typed_protos[TaKind::U8.index()]);

    // DataView
    let dvp = &r.data_view_proto;
    r.ctor("DataView", 1, dv_ctor, dvp);
    r.getter(dvp, "buffer", dv_buffer);
    r.getter(dvp, "byteLength", dv_byte_length);
    r.getter(dvp, "byteOffset", dv_byte_offset);
    r.tag(dvp, "DataView");
    let dv: &[(&str, &str, NativeFn, NativeFn)] = &[
        ("getInt8", "setInt8", dv_get_i8, dv_set_i8),
        ("getUint8", "setUint8", dv_get_u8, dv_set_u8),
        ("getInt16", "setInt16", dv_get_i16, dv_set_i16),
        ("getUint16", "setUint16", dv_get_u16, dv_set_u16),
        ("getInt32", "setInt32", dv_get_i32, dv_set_i32),
        ("getUint32", "setUint32", dv_get_u32, dv_set_u32),
        ("getFloat16", "setFloat16", dv_get_f16, dv_set_f16),
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

/// SpeciesConstructor(o, padrão).
fn species_ctor(vm: &mut Vm, o: &ObjRef, default: &ObjRef) -> Result<ObjRef, Value> {
    let c = match vm.get_obj(o, &PropKey::str("constructor"))? {
        Value::Undefined => return Ok(default.clone()),
        Value::Object(c) => c,
        _ => return Err(vm.type_error("The .constructor property is not an object")),
    };
    let key = PropKey::Sym(vm.realm.sym_species.clone());
    match vm.get_obj(&c, &key)? {
        Value::Undefined | Value::Null => Ok(default.clone()),
        Value::Object(s) if s.is_constructor() => Ok(s),
        _ => Err(vm.type_error("object.constructor[Symbol.species] is not a constructor")),
    }
}

/// Valores de um iterável a partir do método @@iterator já lido.
fn iterate_method(vm: &mut Vm, obj: &Value, method: &Value) -> Result<Vec<Value>, Value> {
    if let Value::Object(o) = obj
        && vm.has_default_array_iterator(o)
    {
        return vm.iterate_to_vec(obj);
    }
    let it = vm.call(method, obj.clone(), Vec::new())?;
    if !matches!(it, Value::Object(_)) {
        return Err(vm.type_error("Result of the Symbol.iterator method is not an object"));
    }
    let next = vm.get_str(&it, "next")?;
    let rec = ObjRef::new(Object::new(
        None,
        Kind::IterRecord {
            iter: it,
            next,
            done: false,
            sync_wrapped: false,
        },
    ));
    let mut out = Vec::new();
    while let Some(x) = vm.iter_step(&rec)? {
        out.push(x);
    }
    Ok(out)
}

// ------------------------------------------------------------ ArrayBuffer

pub fn new_buffer(vm: &mut Vm, len: usize) -> Result<ObjRef, Value> {
    if len > MAX_BYTES {
        return Err(vm.range_error("Array buffer allocation failed"));
    }
    Ok(ObjRef::new(Object::new(
        Some(vm.realm.array_buffer_proto.clone()),
        Kind::ArrayBuffer(BufferData::new(vec![0u8; len])),
    )))
}

/// IsDetachedBuffer.
pub fn is_detached(buf: &ObjRef) -> bool {
    matches!(&buf.borrow().kind, Kind::ArrayBuffer(b) if b.detached)
}

/// (bytes, desanexado, máximo) de um ArrayBuffer.
fn buffer_info(o: &ObjRef) -> Option<(Bytes, bool, Option<usize>)> {
    match &o.borrow().kind {
        Kind::ArrayBuffer(b) => Some((b.bytes.clone(), b.detached, b.max)),
        _ => None,
    }
}

fn ab_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return Err(vm.type_error("Constructor ArrayBuffer requires 'new'"));
    };
    let len = to_index(vm, &a.arg(0), "array buffer length")?;
    // opção maxByteLength (buffer redimensionável)
    let max = match a.arg(1) {
        Value::Object(opts) => match vm.get_obj(&opts, &PropKey::str("maxByteLength"))? {
            Value::Undefined => None,
            m => Some(to_index(vm, &m, "array buffer max length")?),
        },
        _ => None,
    };
    if max.is_some_and(|m| len > m) {
        return Err(vm.range_error("Invalid array buffer max length"));
    }
    let def = vm.realm.array_buffer_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    if max.is_some_and(|m| m > MAX_BYTES) {
        return Err(vm.range_error("Invalid array buffer max length"));
    }
    let b = new_buffer(vm, len)?;
    {
        let mut bm = b.borrow_mut();
        bm.proto = Some(proto);
        if let Kind::ArrayBuffer(d) = &mut bm.kind {
            d.max = max;
        }
    }
    Ok(Value::Object(b))
}

fn is_view(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(matches!(&a.arg(0), Value::Object(o) if matches!(o.borrow().kind, Kind::TypedArray(_) | Kind::DataView(_)))))
}

fn this_buffer(vm: &mut Vm, a: &CallArgs, name: &str) -> Result<(ObjRef, Bytes, bool, Option<usize>), Value> {
    if let Value::Object(o) = &a.this
        && let Some((b, detached, max)) = buffer_info(o)
    {
        return Ok((o.clone(), b, detached, max));
    }
    let d = vm.describe(&a.this);
    Err(vm.type_error(format!("Method ArrayBuffer.prototype.{name} called on incompatible receiver {d}")))
}

fn ab_byte_length(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, b, _, _) = this_buffer(vm, a, "byteLength")?;
    let n = b.borrow().len();
    Ok(Value::Number(n as f64))
}

fn ab_max_byte_length(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, b, detached, max) = this_buffer(vm, a, "maxByteLength")?;
    let n = if detached { 0 } else { max.unwrap_or_else(|| b.borrow().len()) };
    Ok(Value::Number(n as f64))
}

fn ab_resizable(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, _, _, max) = this_buffer(vm, a, "resizable")?;
    Ok(Value::Bool(max.is_some()))
}

fn ab_detached(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, _, detached, _) = this_buffer(vm, a, "detached")?;
    Ok(Value::Bool(detached))
}

fn ab_resize(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, _, max) = this_buffer(vm, a, "resize")?;
    let Some(max) = max else {
        let d = vm.describe(&a.this);
        return Err(vm.type_error(format!("Method ArrayBuffer.prototype.resize called on incompatible receiver {d}")));
    };
    let n = to_index(vm, &a.arg(0), "length")?;
    let Some((bytes, detached, _)) = buffer_info(&o) else { return Ok(Value::Undefined) };
    if detached {
        return Err(vm.type_error("Cannot perform ArrayBuffer.prototype.resize on a detached ArrayBuffer"));
    }
    if n > max {
        return Err(vm.range_error("ArrayBuffer.prototype.resize: Invalid length parameter"));
    }
    bytes.borrow_mut().resize(n, 0);
    Ok(Value::Undefined)
}

fn ab_slice(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, b, detached, _) = this_buffer(vm, a, "slice")?;
    if detached {
        return Err(vm.type_error("Cannot perform ArrayBuffer.prototype.slice on a detached ArrayBuffer"));
    }
    let len = b.borrow().len();
    let first = rel_index(vm, &a.arg(0), len, 0)?;
    let fin = rel_index(vm, &a.arg(1), len, len)?;
    let new_len = fin.saturating_sub(first);
    let Some(def) = vm.realm.array_buffer_ctor.borrow().clone() else { return Ok(Value::Undefined) };
    let c = species_ctor(vm, &o, &def)?;
    let nb = if c.ptr_eq(&def) {
        new_buffer(vm, new_len)?
    } else {
        let r = vm.construct(&c, vec![Value::Number(new_len as f64)], &c)?;
        match &r {
            Value::Object(n) if buffer_info(n).is_some() => n.clone(),
            _ => return Err(vm.type_error("Method ArrayBuffer.prototype.slice called on incompatible receiver")),
        }
    };
    let (to, nd, _) = buffer_info(&nb).unwrap_or_default();
    if nd {
        return Err(vm.type_error("Cannot perform ArrayBuffer.prototype.slice on a detached ArrayBuffer"));
    }
    if nb.ptr_eq(&o) {
        return Err(vm.type_error("ArrayBuffer subclass returned this from species constructor"));
    }
    if to.borrow().len() < new_len {
        return Err(vm.type_error("Derived ArrayBuffer constructor created a buffer which was too small"));
    }
    // o construtor pode ter desanexado ou encolhido o original
    if is_detached(&o) {
        return Err(vm.type_error("Cannot perform ArrayBuffer.prototype.slice on a detached ArrayBuffer"));
    }
    let from = b.borrow();
    if first < from.len() {
        let count = new_len.min(from.len() - first);
        to.borrow_mut()[..count].copy_from_slice(&from[first..first + count]);
    }
    Ok(Value::Object(nb))
}

/// ArrayBufferCopyAndDetach: os bytes passam para um buffer novo.
fn copy_and_detach(vm: &mut Vm, a: &CallArgs, preserve: bool, name: &str) -> Result<Value, Value> {
    let (o, b, _, max) = this_buffer(vm, a, name)?;
    let new_len = if a.arg(0).is_undefined() { b.borrow().len() } else { to_index(vm, &a.arg(0), "array buffer length")? };
    if is_detached(&o) {
        return Err(vm.type_error(format!("Cannot perform ArrayBuffer.prototype.{name} on a detached ArrayBuffer")));
    }
    let new_max = if preserve { max } else { None };
    if new_max.is_some_and(|m| new_len > m) {
        return Err(vm.range_error("Invalid array buffer length"));
    }
    if new_len > MAX_BYTES {
        return Err(vm.range_error("Array buffer allocation failed"));
    }
    let mut bytes = std::mem::take(&mut *b.borrow_mut());
    bytes.resize(new_len, 0);
    if let Kind::ArrayBuffer(d) = &mut o.borrow_mut().kind {
        d.detached = true;
    }
    let mut data = BufferData::new(bytes);
    data.max = new_max;
    Ok(Value::Object(ObjRef::new(Object::new(Some(vm.realm.array_buffer_proto.clone()), Kind::ArrayBuffer(data)))))
}

fn ab_transfer(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    copy_and_detach(vm, a, true, "transfer")
}

fn ab_transfer_fixed(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    copy_and_detach(vm, a, false, "transferToFixedLength")
}

// ------------------------------------------------------------ typed arrays

fn kind_of_ctor(f: &ObjRef) -> TaKind {
    match native_data(f) {
        Value::Number(n) => TaKind::ALL[n as usize],
        _ => TaKind::U8,
    }
}

/// Tamanho de uma vista (elementos; DataView: bytes); None se estiver fora
/// dos limites do buffer ou se ele foi desanexado.
fn view_len(t: &TypedArrayData) -> Option<usize> {
    if is_detached(&t.buffer) {
        return None;
    }
    t.len_in(t.data.borrow().len())
}

/// Estado de um typed array: None se `o` não for um; Some(None) fora dos
/// limites (ou desanexado); Some(Some(tamanho)).
pub fn ta_state(o: &ObjRef) -> Option<Option<usize>> {
    match &o.borrow().kind {
        Kind::TypedArray(t) => Some(view_len(t)),
        _ => None,
    }
}

/// IsTypedArrayFixedLength (outros objetos: true).
pub fn is_fixed_length(o: &ObjRef) -> bool {
    match &o.borrow().kind {
        Kind::TypedArray(t) => !t.tracking && !matches!(&t.buffer.borrow().kind, Kind::ArrayBuffer(b) if b.max.is_some()),
        _ => true,
    }
}

fn make_view(kind: TaKind, buffer: ObjRef, offset: usize, length: usize, tracking: bool, proto: ObjRef) -> ObjRef {
    let data = match &buffer.borrow().kind {
        Kind::ArrayBuffer(b) => b.bytes.clone(),
        _ => Bytes::default(),
    };
    ObjRef::new(Object::new(
        Some(proto),
        Kind::TypedArray(Box::new(TypedArrayData {
            data,
            buffer,
            kind,
            offset,
            length,
            tracking,
        })),
    ))
}

/// Cria um typed array novo (com buffer próprio zerado).
pub fn new_typed(vm: &mut Vm, kind: TaKind, length: usize, proto: Option<ObjRef>) -> Result<ObjRef, Value> {
    let bytes = match length.checked_mul(kind.size()) {
        Some(b) if b <= MAX_BYTES => b,
        _ => return Err(vm.range_error(format!("Invalid typed array length: {length}"))),
    };
    let buf = new_buffer(vm, bytes)?;
    let proto = proto.unwrap_or_else(|| vm.realm.typed_protos[kind.index()].clone());
    Ok(make_view(kind, buf, 0, length, false, proto))
}

fn ta_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let kind = kind_of_ctor(&a.callee);
    let Some(nt) = a.new_target.clone() else {
        return Err(vm.type_error(format!("Constructor {} requires 'new'", kind.name())));
    };
    let def = vm.realm.typed_protos[kind.index()].clone();
    let first = a.arg(0);
    let Value::Object(src) = &first else {
        let len = to_index(vm, &first, "typed array length")?;
        let proto = vm.proto_from_ctor(Some(&nt), &def)?;
        return Ok(Value::Object(new_typed(vm, kind, len, Some(proto))?));
    };
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let (is_ta, is_buf) = match &src.borrow().kind {
        Kind::TypedArray(_) => (true, false),
        Kind::ArrayBuffer(_) => (false, true),
        _ => (false, false),
    };
    if is_ta {
        return Ok(Value::Object(init_from_typed(vm, kind, src, proto)?));
    }
    if is_buf {
        return Ok(Value::Object(init_from_buffer(vm, kind, src, &a.arg(1), &a.arg(2), proto)?));
    }
    // iterável ou array-like
    let key = PropKey::Sym(vm.realm.sym_iterator.clone());
    let method = vm.get_obj(src, &key)?;
    if !method.is_nullish() {
        if !method.is_callable() {
            return Err(vm.type_error("object is not iterable (cannot read property Symbol(Symbol.iterator))"));
        }
        let values = iterate_method(vm, &first, &method)?;
        let t = new_typed(vm, kind, values.len(), Some(proto))?;
        for (i, v) in values.iter().enumerate() {
            vm.typed_set(&t, i, v)?;
        }
        return Ok(Value::Object(t));
    }
    let len = vm.length_of(src)?;
    let t = new_typed(vm, kind, len, Some(proto))?;
    for i in 0..len {
        let v = vm.get_obj(src, &index_key(i))?;
        vm.typed_set(&t, i, &v)?;
    }
    Ok(Value::Object(t))
}

/// InitializeTypedArrayFromArrayBuffer.
fn init_from_buffer(vm: &mut Vm, kind: TaKind, buf: &ObjRef, off: &Value, len: &Value, proto: ObjRef) -> Result<ObjRef, Value> {
    let size = kind.size();
    let offset = to_index(vm, off, "typed array offset")?;
    if offset % size != 0 {
        return Err(vm.range_error(format!("start offset of {} should be a multiple of {size}", kind.name())));
    }
    let new_len = if len.is_undefined() { None } else { Some(to_index(vm, len, "typed array length")?) };
    let Some((bytes, detached, max)) = buffer_info(buf) else { return Err(vm.type_error("not an ArrayBuffer")) };
    if detached {
        return Err(vm.type_error("Cannot perform Construct on a detached ArrayBuffer"));
    }
    let blen = bytes.borrow().len();
    match new_len {
        None if max.is_some() => {
            if offset > blen {
                return Err(vm.range_error(format!("Start offset {offset} is outside the bounds of the buffer")));
            }
            Ok(make_view(kind, buf.clone(), offset, 0, true, proto))
        }
        None => {
            if blen % size != 0 {
                return Err(vm.range_error(format!("byte length of {} should be a multiple of {size}", kind.name())));
            }
            if offset > blen {
                return Err(vm.range_error(format!("Start offset {offset} is outside the bounds of the buffer")));
            }
            Ok(make_view(kind, buf.clone(), offset, (blen - offset) / size, false, proto))
        }
        Some(l) => {
            if l.checked_mul(size).and_then(|b| b.checked_add(offset)).is_none_or(|end| end > blen) {
                return Err(vm.range_error(format!("Invalid typed array length: {l}")));
            }
            Ok(make_view(kind, buf.clone(), offset, l, false, proto))
        }
    }
}

/// InitializeTypedArrayFromTypedArray.
fn init_from_typed(vm: &mut Vm, kind: TaKind, src: &ObjRef, proto: ObjRef) -> Result<ObjRef, Value> {
    let (src_kind, src_len, src_data, src_off) = match &src.borrow().kind {
        Kind::TypedArray(t) => (t.kind, view_len(t), t.data.clone(), t.offset),
        _ => return Err(vm.type_error("this is not a typed array.")),
    };
    let Some(len) = src_len else {
        return Err(vm.type_error("Cannot perform Construct on a detached ArrayBuffer"));
    };
    let t = new_typed(vm, kind, len, Some(proto))?;
    if src_kind.is_bigint() != kind.is_bigint() {
        return Err(vm.type_error("Cannot mix BigInt and other types, use explicit conversions"));
    }
    copy_elements(&src_data, src_off, src_kind, &t, 0, len);
    Ok(t)
}

/// Copia `n` elementos de bytes (`from`, tipo `sk`) para o typed array `to`
/// a partir do elemento `at`, convertendo o tipo se preciso. Os tipos têm o
/// mesmo "content type" (ambos BigInt ou ambos Number).
fn copy_elements(from: &Bytes, from_off: usize, sk: TaKind, to: &ObjRef, at: usize, n: usize) {
    let b = to.borrow();
    let Kind::TypedArray(t) = &b.kind else { return };
    let tk = t.kind;
    let (ss, ts) = (sk.size(), tk.size());
    let src_range = from_off..from_off + n * ss;
    let dst = t.offset + at * ts;
    if Rc::ptr_eq(from, &t.data) {
        let mut d = t.data.borrow_mut();
        if src_range.end > d.len() || dst + n * ts > d.len() {
            return;
        }
        if sk == tk {
            d.copy_within(src_range, dst);
            return;
        }
        // mesmo buffer, tipos diferentes: copia a origem antes
        let tmp = d[src_range].to_vec();
        convert_into(&tmp, sk, &mut d[dst..dst + n * ts], tk);
        return;
    }
    let s = from.borrow();
    let mut d = t.data.borrow_mut();
    if src_range.end > s.len() || dst + n * ts > d.len() {
        return;
    }
    if sk == tk {
        d[dst..dst + n * ts].copy_from_slice(&s[src_range]);
    } else {
        convert_into(&s[src_range], sk, &mut d[dst..dst + n * ts], tk);
    }
}

/// Converte elementos de `src` (tipo `sk`) para `dst` (tipo `tk`).
fn convert_into(src: &[u8], sk: TaKind, dst: &mut [u8], tk: TaKind) {
    let (ss, ts) = (sk.size(), tk.size());
    for (s, d) in src.chunks_exact(ss).zip(dst.chunks_exact_mut(ts)) {
        match sk.decode(s) {
            Value::Number(n) => {
                let mut b = [0u8; 8];
                let k = tk.encode_number(n, &mut b);
                d.copy_from_slice(&b[..k]);
            }
            Value::BigInt(v) => d.copy_from_slice(&v.to_u64_wrapping().to_le_bytes()[..ts]),
            _ => {}
        }
    }
}

/// ValidateTypedArray: `this` precisa ser um typed array dentro dos limites.
/// Devolve (objeto, tipo, tamanho atual).
fn this_ta(vm: &mut Vm, a: &CallArgs, name: &str) -> Result<(ObjRef, TaKind, usize), Value> {
    validate(vm, &a.this, name)
}

fn validate(vm: &mut Vm, v: &Value, name: &str) -> Result<(ObjRef, TaKind, usize), Value> {
    if let Value::Object(o) = v
        && let Kind::TypedArray(t) = &o.borrow().kind
    {
        return match view_len(t) {
            Some(len) => Ok((o.clone(), t.kind, len)),
            None => Err(vm.type_error(format!("Cannot perform {name} on a detached ArrayBuffer"))),
        };
    }
    Err(vm.type_error("this is not a typed array."))
}

/// Tamanho atual de um typed array já validado (TypeError se saiu dos limites).
fn revalidate(vm: &mut Vm, o: &ObjRef, name: &str) -> Result<usize, Value> {
    validate(vm, &Value::Object(o.clone()), name).map(|r| r.2)
}

/// O typed array `this`, sem exigir que esteja dentro dos limites.
fn this_ta_any(vm: &mut Vm, a: &CallArgs) -> Result<ObjRef, Value> {
    if let Value::Object(o) = &a.this
        && matches!(o.borrow().kind, Kind::TypedArray(_))
    {
        return Ok(o.clone());
    }
    Err(vm.type_error("this is not a typed array."))
}

fn elem(o: &ObjRef, i: usize) -> Value {
    elem_opt(o, i).unwrap_or_default()
}

/// Elemento `i` (None se o índice não é válido: HasProperty falso).
fn elem_opt(o: &ObjRef, i: usize) -> Option<Value> {
    match &o.borrow().kind {
        Kind::TypedArray(t) => t.get(i),
        _ => None,
    }
}

/// TypedArrayCreateFromConstructor: o resultado precisa ser um typed array
/// dentro dos limites (e com pelo menos `min` elementos).
fn create_from_ctor(vm: &mut Vm, c: &ObjRef, args: Vec<Value>, min: Option<usize>) -> Result<ObjRef, Value> {
    let r = vm.construct(c, args, c)?;
    let (o, _, len) = validate(vm, &r, "Construct")?;
    if min.is_some_and(|m| len < m) {
        return Err(vm.type_error("Derived TypedArray constructor created an array which was too small"));
    }
    Ok(o)
}

/// TypedArraySpeciesCreate (com `args`; `min` quando é só um tamanho).
fn species_create(vm: &mut Vm, exemplar: &ObjRef, kind: TaKind, args: Vec<Value>, min: Option<usize>) -> Result<ObjRef, Value> {
    let def = vm.realm.typed_ctors.borrow()[kind.index()].clone();
    let c = species_ctor(vm, exemplar, &def)?;
    if let Some(m) = min
        && c.ptr_eq(&def)
    {
        return new_typed(vm, kind, m, None);
    }
    let o = create_from_ctor(vm, &c, args, min)?;
    let big = match &o.borrow().kind {
        Kind::TypedArray(t) => t.kind.is_bigint(),
        _ => false,
    };
    if big != kind.is_bigint() {
        return Err(vm.type_error("Content type of the species constructor result does not match"));
    }
    Ok(o)
}

fn species_create_len(vm: &mut Vm, exemplar: &ObjRef, kind: TaKind, len: usize) -> Result<ObjRef, Value> {
    species_create(vm, exemplar, kind, vec![Value::Number(len as f64)], Some(len))
}

fn get_buffer(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(o) = &a.this
        && let Kind::TypedArray(t) = &o.borrow().kind
    {
        return Ok(Value::Object(t.buffer.clone()));
    }
    Err(vm.type_error("this is not a typed array."))
}

/// (tamanho, offset) para os getters: zero se fora dos limites.
fn ta_getter_info(vm: &mut Vm, a: &CallArgs) -> Result<(usize, usize, TaKind), Value> {
    if let Value::Object(o) = &a.this
        && let Kind::TypedArray(t) = &o.borrow().kind
    {
        return Ok(match view_len(t) {
            Some(len) => (len, t.offset, t.kind),
            None => (0, 0, t.kind),
        });
    }
    Err(vm.type_error("this is not a typed array."))
}

fn get_byte_length(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (len, _, kind) = ta_getter_info(vm, a)?;
    Ok(Value::Number((len * kind.size()) as f64))
}

fn get_byte_offset(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, off, _) = ta_getter_info(vm, a)?;
    Ok(Value::Number(off as f64))
}

fn get_length(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (len, _, _) = ta_getter_info(vm, a)?;
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

fn this_ctor(vm: &mut Vm, a: &CallArgs, name: &str) -> Result<ObjRef, Value> {
    if let Value::Object(c) = &a.this
        && c.is_constructor()
    {
        return Ok(c.clone());
    }
    let d = vm.describe(&a.this);
    Err(vm.type_error(format!("Method %TypedArray%.{name} called on incompatible receiver {d}")))
}

fn ta_from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let c = this_ctor(vm, a, "from")?;
    let map = a.arg(1);
    let mapping = !map.is_undefined();
    if mapping && !map.is_callable() {
        let d = vm.describe(&map);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    let src = a.arg(0);
    let key = PropKey::Sym(vm.realm.sym_iterator.clone());
    let method = vm.get(&src, &key)?;
    if !method.is_nullish() {
        if !method.is_callable() {
            return Err(vm.type_error("object is not iterable (cannot read property Symbol(Symbol.iterator))"));
        }
        let vals = iterate_method(vm, &src, &method)?;
        let len = vals.len();
        let t = create_from_ctor(vm, &c, vec![Value::Number(len as f64)], Some(len))?;
        for (k, v) in vals.into_iter().enumerate() {
            let v = if mapping { vm.call(&map, a.arg(2), vec![v, Value::Number(k as f64)])? } else { v };
            vm.typed_set(&t, k, &v)?;
        }
        return Ok(Value::Object(t));
    }
    let o = vm.to_object(&src)?;
    let len = vm.length_of(&o)?;
    let t = create_from_ctor(vm, &c, vec![Value::Number(len as f64)], Some(len))?;
    for k in 0..len {
        let v = vm.get_obj(&o, &index_key(k))?;
        let v = if mapping { vm.call(&map, a.arg(2), vec![v, Value::Number(k as f64)])? } else { v };
        vm.typed_set(&t, k, &v)?;
    }
    Ok(Value::Object(t))
}

fn ta_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let c = this_ctor(vm, a, "of")?;
    let len = a.args.len();
    let t = create_from_ctor(vm, &c, vec![Value::Number(len as f64)], Some(len))?;
    for (k, v) in a.args.clone().iter().enumerate() {
        vm.typed_set(&t, k, v)?;
    }
    Ok(Value::Object(t))
}

fn callback(vm: &mut Vm, a: &CallArgs) -> Result<Value, Value> {
    let f = a.arg(0);
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    Ok(f)
}

fn make_iter(vm: &mut Vm, a: &CallArgs, kind: IterKind, name: &str) -> Result<Value, Value> {
    let (o, _, _) = this_ta(vm, a, name)?;
    Ok(Value::Object(ObjRef::new(Object::new(
        Some(vm.realm.array_iter_proto.clone()),
        Kind::ArrayIter {
            target: Value::Object(o),
            index: 0,
            kind,
        },
    ))))
}

fn entries(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, IterKind::Entries, "%TypedArray%.prototype.entries")
}

fn keys(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, IterKind::Keys, "%TypedArray%.prototype.keys")
}

fn values(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, IterKind::Values, "%TypedArray%.prototype.values")
}

fn at(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a, "%TypedArray%.prototype.at")?;
    let n = vm.to_integer(&a.arg(0))?;
    let k = if n >= 0.0 { n } else { len as f64 + n };
    if k < 0.0 || k >= len as f64 {
        return Ok(Value::Undefined);
    }
    Ok(elem(&o, k as usize))
}

fn copy_within(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    const NAME: &str = "%TypedArray%.prototype.copyWithin";
    let (o, kind, len) = this_ta(vm, a, NAME)?;
    let to = rel_index(vm, &a.arg(0), len, 0)?;
    let from = rel_index(vm, &a.arg(1), len, 0)?;
    let end = rel_index(vm, &a.arg(2), len, len)?;
    let count = end.saturating_sub(from).min(len - to);
    if count > 0 {
        let len = revalidate(vm, &o, NAME)?;
        let b = o.borrow();
        let Kind::TypedArray(t) = &b.kind else { return Ok(a.this.clone()) };
        let sz = kind.size();
        let limit = len * sz + t.offset;
        let (to_b, from_b, cb) = (to * sz + t.offset, from * sz + t.offset, count * sz);
        let mut d = t.data.borrow_mut();
        if from_b < to_b && to_b < from_b + cb {
            // cópia de trás para frente: só acontece se o fim couber
            if to_b + cb <= limit {
                d.copy_within(from_b..from_b + cb, to_b);
            }
        } else {
            let n = cb.min(limit.saturating_sub(from_b)).min(limit.saturating_sub(to_b));
            if n > 0 {
                d.copy_within(from_b..from_b + n, to_b);
            }
        }
    }
    Ok(a.this.clone())
}

fn iterate_cb(vm: &mut Vm, a: &CallArgs, name: &str, mut f: impl FnMut(&mut Vm, usize, Value, Value) -> Result<bool, Value>) -> Result<(), Value> {
    let (o, _, len) = this_ta(vm, a, name)?;
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
    iterate_cb(vm, a, "%TypedArray%.prototype.every", |_, _, _, r| {
        if !truthy(&r) {
            ok = false;
        }
        Ok(ok)
    })?;
    Ok(Value::Bool(ok))
}

fn some(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut found = false;
    iterate_cb(vm, a, "%TypedArray%.prototype.some", |_, _, _, r| {
        if truthy(&r) {
            found = true;
        }
        Ok(!found)
    })?;
    Ok(Value::Bool(found))
}

fn for_each(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    iterate_cb(vm, a, "%TypedArray%.prototype.forEach", |_, _, _, _| Ok(true))?;
    Ok(Value::Undefined)
}

fn find_generic(vm: &mut Vm, a: &CallArgs, name: &str, rev: bool, want_index: bool) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a, name)?;
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
    find_generic(vm, a, "%TypedArray%.prototype.find", false, false)
}
fn find_index(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, "%TypedArray%.prototype.findIndex", false, true)
}
fn find_last(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, "%TypedArray%.prototype.findLast", true, false)
}
fn find_last_index(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    find_generic(vm, a, "%TypedArray%.prototype.findLastIndex", true, true)
}

/// Converte para o "content type" do tipo (BigInt ou Number).
fn to_content(vm: &mut Vm, kind: TaKind, v: &Value) -> Result<Value, Value> {
    Ok(if kind.is_bigint() {
        Value::BigInt(super::bigint::to_bigint(vm, v)?)
    } else {
        Value::Number(vm.to_number(v)?)
    })
}

/// Bytes de um valor já convertido para o tipo `kind`.
fn encode(kind: TaKind, v: &Value) -> ([u8; 8], usize) {
    let mut b = [0u8; 8];
    let n = match v {
        Value::Number(n) => kind.encode_number(*n, &mut b),
        Value::BigInt(x) => {
            b = x.to_u64_wrapping().to_le_bytes();
            8
        }
        _ => 0,
    };
    (b, n)
}

fn fill(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    const NAME: &str = "%TypedArray%.prototype.fill";
    let (o, kind, len) = this_ta(vm, a, NAME)?;
    let v = to_content(vm, kind, &a.arg(0))?;
    let start = rel_index(vm, &a.arg(1), len, 0)?;
    let end = rel_index(vm, &a.arg(2), len, len)?;
    let len = revalidate(vm, &o, NAME)?;
    let end = end.min(len);
    if start < end {
        let (buf, sz) = encode(kind, &v);
        let b = o.borrow();
        if let Kind::TypedArray(t) = &b.kind {
            let mut d = t.data.borrow_mut();
            let range = t.offset + start * sz..t.offset + end * sz;
            if sz == 1 {
                d[range].fill(buf[0]);
            } else {
                for chunk in d[range].chunks_exact_mut(sz) {
                    chunk.copy_from_slice(&buf[..sz]);
                }
            }
        }
    }
    Ok(a.this.clone())
}

fn filter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a, "%TypedArray%.prototype.filter")?;
    let cb = callback(vm, a)?;
    let mut kept = Vec::new();
    for i in 0..len {
        let v = elem(&o, i);
        let r = vm.call(&cb, a.arg(1), vec![v.clone(), Value::Number(i as f64), a.this.clone()])?;
        if truthy(&r) {
            kept.push(v);
        }
    }
    let t = species_create_len(vm, &o, kind, kept.len())?;
    for (i, v) in kept.iter().enumerate() {
        vm.typed_set(&t, i, v)?;
    }
    Ok(Value::Object(t))
}

fn map(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a, "%TypedArray%.prototype.map")?;
    let cb = callback(vm, a)?;
    let t = species_create_len(vm, &o, kind, len)?;
    for i in 0..len {
        let v = elem(&o, i);
        let r = vm.call(&cb, a.arg(1), vec![v, Value::Number(i as f64), a.this.clone()])?;
        vm.typed_set(&t, i, &r)?;
    }
    Ok(Value::Object(t))
}

fn includes(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a, "%TypedArray%.prototype.includes")?;
    if len == 0 {
        return Ok(Value::Bool(false));
    }
    let start = rel_index(vm, &a.arg(1), len, 0)?;
    let target = a.arg(0);
    Ok(Value::Bool((start..len).any(|i| same_value_zero(&elem(&o, i), &target))))
}

fn index_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a, "%TypedArray%.prototype.indexOf")?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let start = rel_index(vm, &a.arg(1), len, 0)?;
    let target = a.arg(0);
    let found = (start..len).find(|&i| elem_opt(&o, i).is_some_and(|v| strict_equals(&v, &target)));
    Ok(Value::Number(found.map_or(-1.0, |i| i as f64)))
}

fn last_index_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a, "%TypedArray%.prototype.lastIndexOf")?;
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
            n.min((len - 1) as f64) as usize
        }
    } else {
        len - 1
    };
    let target = a.arg(0);
    let found = (0..=start).rev().find(|&i| elem_opt(&o, i).is_some_and(|v| strict_equals(&v, &target)));
    Ok(Value::Number(found.map_or(-1.0, |i| i as f64)))
}

fn join(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a, "%TypedArray%.prototype.join")?;
    let sep = if a.arg(0).is_undefined() { ",".to_string() } else { vm.to_rust_string(&a.arg(0))? };
    let mut out = String::new();
    for i in 0..len {
        if i > 0 {
            out.push_str(&sep);
        }
        let v = elem(&o, i);
        if !v.is_undefined() {
            out.push_str(&vm.to_rust_string(&v)?);
        }
    }
    Ok(Value::str(&out))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a, "%TypedArray%.prototype.toLocaleString")?;
    let mut out = String::new();
    for i in 0..len {
        if i > 0 {
            out.push(',');
        }
        let v = elem(&o, i);
        if v.is_nullish() {
            continue;
        }
        let f = vm.get_str(&v, "toLocaleString")?;
        let s = vm.call(&f, v, Vec::new())?;
        out.push_str(&vm.to_rust_string(&s)?);
    }
    Ok(Value::str(&out))
}

fn reduce_generic(vm: &mut Vm, a: &CallArgs, name: &str, right: bool) -> Result<Value, Value> {
    let (o, _, len) = this_ta(vm, a, name)?;
    let cb = callback(vm, a)?;
    let mut it: Box<dyn Iterator<Item = usize>> = if right { Box::new((0..len).rev()) } else { Box::new(0..len) };
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
    reduce_generic(vm, a, "%TypedArray%.prototype.reduce", false)
}

fn reduce_right(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    reduce_generic(vm, a, "%TypedArray%.prototype.reduceRight", true)
}

fn values_of(o: &ObjRef, len: usize) -> Vec<Value> {
    (0..len).map(|i| elem(o, i)).collect()
}

fn reverse(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a, "%TypedArray%.prototype.reverse")?;
    let b = o.borrow();
    if let Kind::TypedArray(t) = &b.kind {
        let sz = kind.size();
        let mut d = t.data.borrow_mut();
        let s = &mut d[t.offset..t.offset + len * sz];
        if sz == 1 {
            s.reverse();
        } else {
            for i in 0..len / 2 {
                let j = len - 1 - i;
                for k in 0..sz {
                    s.swap(i * sz + k, j * sz + k);
                }
            }
        }
    }
    Ok(a.this.clone())
}

fn set(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    const NAME: &str = "%TypedArray%.prototype.set";
    let o = this_ta_any(vm, a)?;
    let offset = vm.to_integer(&a.arg(1))?;
    if offset < 0.0 {
        return Err(vm.range_error("offset is out of bounds"));
    }
    let src = a.arg(0);
    let src_ta = match &src {
        Value::Object(s) => match &s.borrow().kind {
            Kind::TypedArray(t) => Some((t.kind, view_len(t), t.data.clone(), t.offset)),
            _ => None,
        },
        _ => None,
    };
    if let Some((sk, slen, sdata, soff)) = src_ta {
        // SetTypedArrayFromTypedArray
        let (_, tk, tlen) = validate(vm, &a.this, NAME)?;
        let Some(slen) = slen else {
            return Err(vm.type_error(format!("Cannot perform {NAME} on a detached ArrayBuffer")));
        };
        if offset.is_infinite() || slen as f64 + offset > tlen as f64 {
            return Err(vm.range_error("offset is out of bounds"));
        }
        if sk.is_bigint() != tk.is_bigint() {
            return Err(vm.type_error("Cannot mix BigInt and other types, use explicit conversions"));
        }
        copy_elements(&sdata, soff, sk, &o, offset as usize, slen);
        return Ok(Value::Undefined);
    }
    // SetTypedArrayFromArrayLike
    let (_, _, tlen) = validate(vm, &a.this, NAME)?;
    let s = vm.to_object(&src)?;
    let n = vm.length_of(&s)?;
    if offset.is_infinite() || n as f64 + offset > tlen as f64 {
        return Err(vm.range_error("offset is out of bounds"));
    }
    let offset = offset as usize;
    for i in 0..n {
        let fast = match &s.borrow().kind {
            Kind::Array(v) => v.get(i).filter(|x| !matches!(x, Value::Empty)).cloned(),
            _ => None,
        };
        let v = match fast {
            Some(v) => v,
            None => vm.get_obj(&s, &index_key(i))?,
        };
        vm.typed_set(&o, offset + i, &v)?;
    }
    Ok(Value::Undefined)
}

fn slice(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    const NAME: &str = "%TypedArray%.prototype.slice";
    let (o, kind, len) = this_ta(vm, a, NAME)?;
    let start = rel_index(vm, &a.arg(0), len, 0)?;
    let end = rel_index(vm, &a.arg(1), len, len)?;
    let count = end.saturating_sub(start);
    let t = species_create_len(vm, &o, kind, count)?;
    if count > 0 {
        let len = revalidate(vm, &o, NAME)?;
        let end = end.min(len);
        let count = end.saturating_sub(start);
        let target = match &t.borrow().kind {
            Kind::TypedArray(x) => Some((x.kind, x.data.clone(), x.offset)),
            _ => None,
        };
        let source = match &o.borrow().kind {
            Kind::TypedArray(x) => Some((x.data.clone(), x.offset)),
            _ => None,
        };
        let (Some((tk, tdata, toff)), Some((sdata, soff))) = (target, source) else { return Ok(Value::Object(t)) };
        if tk == kind {
            // mesmo tipo: cópia byte a byte (o destino pode ser o mesmo buffer)
            let sz = kind.size();
            let (from, n) = (soff + start * sz, count * sz);
            if Rc::ptr_eq(&sdata, &tdata) {
                let mut d = sdata.borrow_mut();
                let n = n.min(d.len().saturating_sub(toff)).min(d.len().saturating_sub(from));
                for k in 0..n {
                    d[toff + k] = d[from + k];
                }
            } else {
                let s = sdata.borrow();
                let mut d = tdata.borrow_mut();
                let n = n.min(d.len().saturating_sub(toff)).min(s.len().saturating_sub(from));
                if n > 0 {
                    d[toff..toff + n].copy_from_slice(&s[from..from + n]);
                }
            }
        } else {
            for (n, k) in (start..end).enumerate() {
                let v = elem(&o, k);
                vm.typed_set(&t, n, &v)?;
            }
        }
    }
    Ok(Value::Object(t))
}

fn subarray(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_ta_any(vm, a)?;
    let (buffer, kind, base, tracking, src_len) = match &o.borrow().kind {
        Kind::TypedArray(t) => (t.buffer.clone(), t.kind, t.offset, t.tracking, view_len(t).unwrap_or(0)),
        _ => return Ok(Value::Undefined),
    };
    let start = rel_index(vm, &a.arg(0), src_len, 0)?;
    let begin = base + start * kind.size();
    let mut args = vec![Value::Object(buffer), Value::Number(begin as f64)];
    if !(tracking && a.arg(1).is_undefined()) {
        let end = rel_index(vm, &a.arg(1), src_len, src_len)?;
        args.push(Value::Number(end.saturating_sub(start) as f64));
    }
    Ok(Value::Object(species_create(vm, &o, kind, args, None)?))
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

fn compare_fn(vm: &mut Vm, a: &CallArgs) -> Result<Value, Value> {
    let cmp = a.arg(0);
    if !cmp.is_undefined() && !cmp.is_callable() {
        return Err(vm.type_error("The comparison function must be either a function or undefined"));
    }
    Ok(cmp)
}

fn sorted_values(vm: &mut Vm, o: &ObjRef, len: usize, cmp: &Value) -> Result<Vec<Value>, Value> {
    let mut vals = values_of(o, len);
    if cmp.is_undefined() {
        vals.sort_by(numeric_order);
        return Ok(vals);
    }
    super::array::sort_values(vm, vals, cmp)
}

fn sort(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cmp = compare_fn(vm, a)?;
    let (o, _, len) = this_ta(vm, a, "%TypedArray%.prototype.sort")?;
    let vals = sorted_values(vm, &o, len, &cmp)?;
    for (i, v) in vals.iter().enumerate() {
        vm.typed_set(&o, i, v)?;
    }
    Ok(a.this.clone())
}

fn to_sorted(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cmp = compare_fn(vm, a)?;
    let (o, kind, len) = this_ta(vm, a, "%TypedArray%.prototype.toSorted")?;
    let t = new_typed(vm, kind, len, None)?;
    let vals = sorted_values(vm, &o, len, &cmp)?;
    for (i, v) in vals.iter().enumerate() {
        vm.typed_set(&t, i, v)?;
    }
    Ok(Value::Object(t))
}

fn to_reversed(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a, "%TypedArray%.prototype.toReversed")?;
    let t = new_typed(vm, kind, len, None)?;
    for k in 0..len {
        let v = elem(&o, len - 1 - k);
        vm.typed_set(&t, k, &v)?;
    }
    Ok(Value::Object(t))
}

fn with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (o, kind, len) = this_ta(vm, a, "%TypedArray%.prototype.with")?;
    let n = vm.to_integer(&a.arg(0))?;
    let idx = if n >= 0.0 { n } else { len as f64 + n };
    let v = to_content(vm, kind, &a.arg(1))?;
    if !vm.ta_valid_index(&o, idx) {
        return Err(vm.range_error("Invalid typed array index"));
    }
    let t = new_typed(vm, kind, len, None)?;
    for k in 0..len {
        let x = if k as f64 == idx { v.clone() } else { elem(&o, k) };
        vm.typed_set(&t, k, &x)?;
    }
    Ok(Value::Object(t))
}

// ---------------------------------------------------------------- DataView

fn dv_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    const DETACHED: &str = "Cannot perform DataView constructor on a detached ArrayBuffer";
    let Some(nt) = a.new_target.clone() else {
        return Err(vm.type_error("Constructor DataView requires 'new'"));
    };
    let buf = match a.arg(0) {
        Value::Object(b) if buffer_info(&b).is_some() => b,
        _ => return Err(vm.type_error("First argument to DataView constructor must be an ArrayBuffer")),
    };
    let offset = to_index(vm, &a.arg(1), "DataView offset")?;
    let (data, detached, max) = buffer_info(&buf).unwrap_or_default();
    if detached {
        return Err(vm.type_error(DETACHED));
    }
    let blen = data.borrow().len();
    if offset > blen {
        return Err(vm.range_error(format!("Start offset {offset} is outside the bounds of the buffer")));
    }
    let len_arg = a.arg(2);
    // None: acompanha o tamanho do buffer redimensionável
    let length = if len_arg.is_undefined() {
        if max.is_some() { None } else { Some(blen - offset) }
    } else {
        let l = to_index(vm, &len_arg, "DataView length")?;
        if offset + l > blen {
            return Err(vm.range_error(format!("Invalid DataView length {l}")));
        }
        Some(l)
    };
    let def = vm.realm.data_view_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    // ler o protótipo pode ter desanexado ou encolhido o buffer
    if is_detached(&buf) {
        return Err(vm.type_error(DETACHED));
    }
    let blen = data.borrow().len();
    if offset > blen {
        return Err(vm.range_error(format!("Start offset {offset} is outside the bounds of the buffer")));
    }
    if !len_arg.is_undefined()
        && let Some(l) = length
        && offset + l > blen
    {
        return Err(vm.range_error(format!("Invalid DataView length {l}")));
    }
    Ok(Value::Object(ObjRef::new(Object::new(
        Some(proto),
        Kind::DataView(Box::new(TypedArrayData {
            data,
            buffer: buf,
            kind: TaKind::U8,
            offset,
            length: length.unwrap_or(0),
            tracking: length.is_none(),
        })),
    ))))
}

fn this_dv(vm: &mut Vm, a: &CallArgs, name: &str) -> Result<ObjRef, Value> {
    if let Value::Object(o) = &a.this
        && matches!(o.borrow().kind, Kind::DataView(_))
    {
        return Ok(o.clone());
    }
    let d = vm.describe(&a.this);
    Err(vm.type_error(format!("Method DataView.prototype.{name} called on incompatible receiver {d}")))
}

/// (bytes, offset, tamanho) de um DataView dentro dos limites; TypeError se não.
fn dv_state(vm: &mut Vm, o: &ObjRef, name: &str) -> Result<(Bytes, usize, usize), Value> {
    if let Kind::DataView(t) = &o.borrow().kind
        && let Some(len) = view_len(t)
    {
        return Ok((t.data.clone(), t.offset, len));
    }
    Err(vm.type_error(format!("Cannot perform {name} on a detached ArrayBuffer")))
}

fn dv_buffer(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_dv(vm, a, "buffer")?;
    let b = o.borrow();
    match &b.kind {
        Kind::DataView(t) => Ok(Value::Object(t.buffer.clone())),
        _ => Ok(Value::Undefined),
    }
}

/// (tamanho, offset) de um DataView para os getters: zero se fora dos limites.
fn dv_getter_info(o: &ObjRef) -> (usize, usize) {
    match &o.borrow().kind {
        Kind::DataView(t) => view_len(t).map_or((0, 0), |len| (len, t.offset)),
        _ => (0, 0),
    }
}

fn dv_byte_length(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_dv(vm, a, "byteLength")?;
    Ok(Value::Number(dv_getter_info(&o).0 as f64))
}

fn dv_byte_offset(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_dv(vm, a, "byteOffset")?;
    Ok(Value::Number(dv_getter_info(&o).1 as f64))
}

/// GetViewValue.
fn dv_get(vm: &mut Vm, a: &CallArgs, kind: TaKind, name: &str) -> Result<Value, Value> {
    let o = this_dv(vm, a, name)?;
    let pos = to_index(vm, &a.arg(0), "offset")?;
    let little = truthy(&a.arg(1));
    let (data, off, len) = dv_state(vm, &o, &format!("DataView.prototype.{name}"))?;
    let n = kind.size();
    if pos + n > len {
        return Err(vm.range_error("Offset is outside the bounds of the DataView"));
    }
    let mut b = [0u8; 8];
    b[..n].copy_from_slice(&data.borrow()[off + pos..off + pos + n]);
    if !little {
        b[..n].reverse();
    }
    Ok(kind.decode(&b))
}

/// SetViewValue.
fn dv_set(vm: &mut Vm, a: &CallArgs, kind: TaKind, name: &str) -> Result<Value, Value> {
    let o = this_dv(vm, a, name)?;
    let pos = to_index(vm, &a.arg(0), "offset")?;
    let v = to_content(vm, kind, &a.arg(1))?;
    let little = truthy(&a.arg(2));
    let (data, off, len) = dv_state(vm, &o, &format!("DataView.prototype.{name}"))?;
    let (mut b, n) = encode(kind, &v);
    if pos + n > len {
        return Err(vm.range_error("Offset is outside the bounds of the DataView"));
    }
    if !little {
        b[..n].reverse();
    }
    data.borrow_mut()[off + pos..off + pos + n].copy_from_slice(&b[..n]);
    Ok(Value::Undefined)
}

macro_rules! dv_methods {
    ($get:ident, $set:ident, $kind:expr, $gname:expr, $sname:expr) => {
        fn $get(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
            dv_get(vm, a, $kind, $gname)
        }
        fn $set(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
            dv_set(vm, a, $kind, $sname)
        }
    };
}

dv_methods!(dv_get_i8, dv_set_i8, TaKind::I8, "getInt8", "setInt8");
dv_methods!(dv_get_u8, dv_set_u8, TaKind::U8, "getUint8", "setUint8");
dv_methods!(dv_get_i16, dv_set_i16, TaKind::I16, "getInt16", "setInt16");
dv_methods!(dv_get_u16, dv_set_u16, TaKind::U16, "getUint16", "setUint16");
dv_methods!(dv_get_i32, dv_set_i32, TaKind::I32, "getInt32", "setInt32");
dv_methods!(dv_get_u32, dv_set_u32, TaKind::U32, "getUint32", "setUint32");
dv_methods!(dv_get_f16, dv_set_f16, TaKind::F16, "getFloat16", "setFloat16");
dv_methods!(dv_get_f32, dv_set_f32, TaKind::F32, "getFloat32", "setFloat32");
dv_methods!(dv_get_f64, dv_set_f64, TaKind::F64, "getFloat64", "setFloat64");
dv_methods!(dv_get_bi64, dv_set_bi64, TaKind::BI64, "getBigInt64", "setBigInt64");
dv_methods!(dv_get_bu64, dv_set_bu64, TaKind::BU64, "getBigUint64", "setBigUint64");
