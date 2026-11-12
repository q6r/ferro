//! Objetos embutidos (Object, Array, String, Math, JSON, console...).

pub mod array;
pub mod async_generator;
pub mod bigint;
pub mod collections;
pub mod console;
pub mod date;
pub mod error;
pub mod function;
pub mod generator;
pub mod global;
pub mod intl;
pub mod json;
pub mod number;
pub mod object;
pub mod promise;
pub mod proxy;
pub mod reflect;
pub mod regexp;
pub mod strings;
pub mod symbol;
pub mod timers;
pub mod typed;

use crate::string::JsStr;
use crate::value::*;
use crate::vm::Vm;

/// Os objetos intrínsecos de um "mundo" JavaScript.
pub struct Realm {
    pub global: ObjRef,
    pub object_proto: ObjRef,
    pub function_proto: ObjRef,
    pub array_proto: ObjRef,
    pub string_proto: ObjRef,
    pub number_proto: ObjRef,
    pub boolean_proto: ObjRef,
    pub symbol_proto: ObjRef,
    pub error_proto: ObjRef,
    pub type_error_proto: ObjRef,
    pub range_error_proto: ObjRef,
    pub reference_error_proto: ObjRef,
    pub syntax_error_proto: ObjRef,
    pub eval_error_proto: ObjRef,
    pub uri_error_proto: ObjRef,
    pub iterator_proto: ObjRef,
    pub array_iter_proto: ObjRef,
    pub map_proto: ObjRef,
    pub set_proto: ObjRef,
    pub weakmap_proto: ObjRef,
    pub weakset_proto: ObjRef,
    pub map_iter_proto: ObjRef,
    pub set_iter_proto: ObjRef,
    pub string_iter_proto: ObjRef,
    pub regexp_proto: ObjRef,
    pub date_proto: ObjRef,
    pub generator_proto: ObjRef,
    pub promise_proto: ObjRef,
    pub bigint_proto: ObjRef,
    pub array_buffer_proto: ObjRef,
    pub typed_array_proto: ObjRef,
    /// Um protótipo por tipo de elemento, na ordem de `TaKind::ALL`.
    pub typed_protos: Vec<ObjRef>,
    pub data_view_proto: ObjRef,
    pub async_function_proto: ObjRef,
    pub generator_function_proto: ObjRef,
    pub async_generator_function_proto: ObjRef,
    pub async_generator_proto: ObjRef,
    pub async_iterator_proto: ObjRef,
    /// %ArrayIteratorPrototype%.next (caminho rápido do for-of).
    pub array_iter_next: ObjRef,
    /// Array.prototype.values.
    pub array_values: ObjRef,
    /// Function.prototype[Symbol.hasInstance].
    pub function_has_instance: ObjRef,
    /// RegExp.prototype.exec original (atalho quando não foi trocado).
    pub regexp_exec: ObjRef,
    pub regexp_string_iter_proto: ObjRef,
    pub sym_iterator: Sym,
    pub sym_async_iterator: Sym,
    pub sym_has_instance: Sym,
    pub sym_to_primitive: Sym,
    pub sym_to_string_tag: Sym,
    pub sym_species: Sym,
    pub sym_is_concat_spreadable: Sym,
    pub sym_unscopables: Sym,
    pub sym_match: Sym,
    pub sym_match_all: Sym,
    pub sym_replace: Sym,
    pub sym_search: Sym,
    pub sym_split: Sym,
}

fn plain(proto: Option<&ObjRef>) -> ObjRef {
    ObjRef::new(Object::new(proto.cloned(), Kind::Ordinary))
}

fn sym(desc: &str) -> Sym {
    Sym::new(Some(JsStr::new(desc)))
}

fn noop(_: &mut Vm, _: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Undefined)
}

impl Realm {
