//! O resto da plataforma web em Rust: rede, URL, armazenamento, cookies,
//! base64, medidas da janela. A maior parte fica num objeto `__native` que
//! o prelúdio usa e depois esconde.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use ferro_js::value::{CallArgs, Kind, ObjRef, PropKey, TaKind};
use ferro_js::{Value, Vm};
use url::Url;

use super::{Done, Host, host, node_of};
use crate::net;

type R = Result<Value, Value>;

fn s(v: &str) -> Value {
    Value::str(v)
}

fn read<T>(vm: &Vm, f: impl FnOnce(&Host) -> T) -> T {
    let h = host(vm);
    let b = h.borrow();
    f(&b)
}

fn write<T>(vm: &Vm, f: impl FnOnce(&mut Host) -> T) -> T {
    let h = host(vm);
    let mut b = h.borrow_mut();
    f(&mut b)
}

fn arg_string(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<String, Value> {
    vm.to_rust_string(&a.arg(i))
}

fn set(vm: &mut Vm, o: &ObjRef, k: &str, v: Value) {
    vm.define_data(o, PropKey::str(k), v);
}

// ------------------------------------------------------------------ bytes

/// Os bytes de um TypedArray, ArrayBuffer ou DataView.
pub(crate) fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    let Value::Object(o) = v else { return None };
    let b = o.borrow();
    match &b.kind {
        Kind::ArrayBuffer(bytes) => Some(bytes.borrow().clone()),
        Kind::TypedArray(t) | Kind::DataView(t) => {
            let data = t.data.borrow();
            let len = if matches!(b.kind, Kind::DataView(_)) { t.length } else { t.length * t.kind.size() };
            let end = (t.offset + len).min(data.len());
            Some(data[t.offset.min(end)..end].to_vec())
        }
        _ => None,
    }
}

