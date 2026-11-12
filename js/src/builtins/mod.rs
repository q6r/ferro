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
pub mod iterator;
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
    /// %IteratorHelperPrototype% (map, filter, Iterator.concat...).
    pub iterator_helper_proto: ObjRef,
    /// %WrapForValidIteratorPrototype% (Iterator.from).
    pub wrap_for_valid_iter_proto: ObjRef,
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
    /// %AsyncFromSyncIteratorPrototype% (for await / yield* sobre iterável síncrono).
    pub async_from_sync_iter_proto: ObjRef,
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
    pub sym_dispose: Sym,
    pub sym_async_dispose: Sym,
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
    pub fn new() -> Realm {
        let object_proto = plain(None);
        let function_proto = ObjRef::new(Object::new(
            Some(object_proto.clone()),
            Kind::Function(Func::Native(Native {
                f: noop,
                ctor: false,
                data: Value::Undefined,
            })),
        ));
        let p = Some(&object_proto);
        let error_proto = plain(p);
        let iterator_proto = plain(p);
        let ip = Some(&iterator_proto);
        let async_iterator_proto = plain(p);
        let typed_array_proto = plain(p);
        let mk_native = |name: &str, len: u32, f: NativeFn| native_fn(&function_proto, name, len, f, false);
        let array_iter_next = mk_native("next", 0, array::array_iter_next);
        let array_values = mk_native("values", 0, array::values);
        let function_has_instance = mk_native("[Symbol.hasInstance]", 1, function::has_instance);
        let regexp_exec = regexp::exec_native(&function_proto);
        let realm = Realm {
            global: plain(p),
            array_proto: ObjRef::new(Object::new(Some(object_proto.clone()), Kind::Array(Vec::new()))),
            string_proto: ObjRef::new(Object::new(Some(object_proto.clone()), Kind::String(JsStr::empty()))),
            number_proto: ObjRef::new(Object::new(Some(object_proto.clone()), Kind::Number(0.0))),
            boolean_proto: ObjRef::new(Object::new(Some(object_proto.clone()), Kind::Boolean(false))),
            symbol_proto: plain(p),
            type_error_proto: plain(Some(&error_proto)),
            range_error_proto: plain(Some(&error_proto)),
            reference_error_proto: plain(Some(&error_proto)),
            syntax_error_proto: plain(Some(&error_proto)),
            eval_error_proto: plain(Some(&error_proto)),
            uri_error_proto: plain(Some(&error_proto)),
            iterator_helper_proto: plain(ip),
            wrap_for_valid_iter_proto: plain(ip),
            array_iter_proto: plain(ip),
            map_proto: plain(p),
            set_proto: plain(p),
            weakmap_proto: plain(p),
            weakset_proto: plain(p),
            map_iter_proto: plain(ip),
            set_iter_proto: plain(ip),
            string_iter_proto: plain(ip),
            regexp_proto: plain(p),
            date_proto: plain(p),
            generator_proto: plain(ip),
            promise_proto: plain(p),
            bigint_proto: plain(p),
            array_buffer_proto: plain(p),
            typed_protos: TaKind::ALL.iter().map(|_| plain(Some(&typed_array_proto))).collect(),
            typed_array_proto,
            data_view_proto: plain(p),
            async_function_proto: plain(Some(&function_proto)),
            generator_function_proto: plain(Some(&function_proto)),
            async_generator_function_proto: plain(Some(&function_proto)),
            async_generator_proto: plain(Some(&async_iterator_proto)),
            async_from_sync_iter_proto: plain(Some(&async_iterator_proto)),
            async_iterator_proto,
            array_iter_next,
            array_values,
            function_has_instance,
            regexp_exec,
            regexp_string_iter_proto: plain(ip),
            sym_iterator: sym("Symbol.iterator"),
            sym_async_iterator: sym("Symbol.asyncIterator"),
            sym_has_instance: sym("Symbol.hasInstance"),
            sym_to_primitive: sym("Symbol.toPrimitive"),
            sym_to_string_tag: sym("Symbol.toStringTag"),
            sym_species: sym("Symbol.species"),
            sym_is_concat_spreadable: sym("Symbol.isConcatSpreadable"),
            sym_unscopables: sym("Symbol.unscopables"),
            sym_match: sym("Symbol.match"),
            sym_match_all: sym("Symbol.matchAll"),
            sym_replace: sym("Symbol.replace"),
            sym_search: sym("Symbol.search"),
            sym_split: sym("Symbol.split"),
            sym_dispose: sym("Symbol.dispose"),
            sym_async_dispose: sym("Symbol.asyncDispose"),
            error_proto,
            iterator_proto,
            object_proto,
            function_proto,
        };
        object::install(&realm);
        function::install(&realm);
        array::install(&realm);
        strings::install(&realm);
        number::install(&realm);
        symbol::install(&realm);
        error::install(&realm);
        json::install(&realm);
        collections::install(&realm);
        global::install(&realm);
        console::install(&realm);
        reflect::install(&realm);
        date::install(&realm);
        regexp::install(&realm);
        generator::install(&realm);
        iterator::install(&realm);
        promise::install(&realm);
        async_generator::install(&realm);
        timers::install(&realm);
        bigint::install(&realm);
        typed::install(&realm);
        proxy::install(&realm);
        intl::install(&realm);
        realm
    }

    pub fn native(&self, name: &str, len: u32, f: NativeFn) -> ObjRef {
        native_fn(&self.function_proto, name, len, f, false)
    }

    /// Método não enumerável.
    pub fn method(&self, target: &ObjRef, name: &str, len: u32, f: NativeFn) -> ObjRef {
        let fo = self.native(name, len, f);
        target.borrow_mut().props.insert(PropKey::str(name), Property::hidden(Value::Object(fo.clone())));
        fo
    }

    pub fn method_sym(&self, target: &ObjRef, s: &Sym, name: &str, len: u32, f: NativeFn) -> ObjRef {
        let fo = self.native(name, len, f);
        target.borrow_mut().props.insert(PropKey::Sym(s.clone()), Property::hidden(Value::Object(fo.clone())));
        fo
    }

    pub fn getter(&self, target: &ObjRef, name: &str, f: NativeFn) {
        let fo = self.native(&format!("get {name}"), 0, f);
        target.borrow_mut().props.insert(
            PropKey::str(name),
            Property {
                slot: Slot::Accessor { get: Some(fo), set: None },
                enumerable: false,
                configurable: true,
            },
        );
    }

    /// Propriedade acessora (getter e/ou setter nativos), não enumerável.
    pub fn accessor(&self, target: &ObjRef, name: &str, get: Option<NativeFn>, set: Option<NativeFn>) {
        let g = get.map(|f| self.native(&format!("get {name}"), 0, f));
        let s = set.map(|f| self.native(&format!("set {name}"), 1, f));
        target.borrow_mut().props.insert(
            PropKey::str(name),
            Property {
                slot: Slot::Accessor { get: g, set: s },
                enumerable: true,
                configurable: true,
            },
        );
    }

    pub fn getter_sym(&self, target: &ObjRef, s: &Sym, name: &str, f: NativeFn) {
        let fo = self.native(&format!("get {name}"), 0, f);
        target.borrow_mut().props.insert(
            PropKey::Sym(s.clone()),
            Property {
                slot: Slot::Accessor { get: Some(fo), set: None },
                enumerable: false,
                configurable: true,
            },
        );
    }

    /// Construtor global ligado ao seu protótipo.
    pub fn ctor(&self, name: &str, len: u32, f: NativeFn, proto: &ObjRef) -> ObjRef {
        let c = native_fn(&self.function_proto, name, len, f, true);
        c.borrow_mut().props.insert(
            PropKey::str("prototype"),
            Property {
                slot: Slot::Data {
                    value: Value::Object(proto.clone()),
                    writable: false,
                },
                enumerable: false,
                configurable: false,
            },
        );
        proto.borrow_mut().props.insert(PropKey::str("constructor"), Property::hidden(Value::Object(c.clone())));
        self.global.borrow_mut().props.insert(PropKey::str(name), Property::hidden(Value::Object(c.clone())));
        c
    }

    /// Valor constante (não gravável, não enumerável).
    pub fn constant(&self, target: &ObjRef, name: &str, v: Value) {
        target.borrow_mut().props.insert(
            PropKey::str(name),
            Property {
                slot: Slot::Data { value: v, writable: false },
                enumerable: false,
                configurable: false,
            },
        );
    }

    pub fn tag(&self, target: &ObjRef, tag: &str) {
        target.borrow_mut().props.insert(
            PropKey::Sym(self.sym_to_string_tag.clone()),
            Property {
                slot: Slot::Data {
                    value: Value::str(tag),
                    writable: false,
                },
                enumerable: false,
                configurable: true,
            },
        );
    }

    pub fn global_value(&self, name: &str, v: Value) {
        self.global.borrow_mut().props.insert(PropKey::str(name), Property::hidden(v));
    }
}

impl Default for Realm {
    fn default() -> Self {
        Self::new()
    }
}

pub fn native_fn(function_proto: &ObjRef, name: &str, len: u32, f: NativeFn, ctor: bool) -> ObjRef {
    let o = ObjRef::new(Object::new(
        Some(function_proto.clone()),
        Kind::Function(Func::Native(Native {
            f,
            ctor,
            data: Value::Undefined,
        })),
    ));
    {
        let mut b = o.borrow_mut();
        b.props.insert(PropKey::str("length"), crate::ops::readonly(Value::Number(len as f64)));
        b.props.insert(PropKey::str("name"), crate::ops::readonly(Value::str(name)));
    }
    o
}

// ---------------------------------------------------- auxiliares comuns

/// Dados guardados numa função nativa (ex.: alvo de um Proxy, resolve de Promise).
pub fn native_data(f: &ObjRef) -> Value {
    match &f.borrow().kind {
        Kind::Function(Func::Native(n)) => n.data.clone(),
        _ => Value::Undefined,
    }
}

pub fn set_native_data(f: &ObjRef, v: Value) {
    if let Kind::Function(Func::Native(n)) = &mut f.borrow_mut().kind {
        n.data = v;
    }
}

pub fn arg_num(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<f64, Value> {
    vm.to_number(&a.arg(i))
}

pub fn arg_str(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<JsStr, Value> {
    vm.to_string(&a.arg(i))
}

/// `this` convertido para objeto.
pub fn this_obj(vm: &mut Vm, a: &CallArgs) -> Result<ObjRef, Value> {
    vm.to_object(&a.this)
}

/// Converte um índice relativo (aceita negativos e undefined = padrão).
pub fn rel_index(vm: &mut Vm, v: &Value, len: usize, default: usize) -> Result<usize, Value> {
    if v.is_undefined() {
        return Ok(default);
    }
    let n = vm.to_integer(v)?;
    Ok(crate::conv::relative_index(n, len))
}

pub fn obj_value(o: ObjRef) -> Value {
    Value::Object(o)
}
