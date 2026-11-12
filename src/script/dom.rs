//! Node, Element, Document e companhia, em Rust, direto sobre a arena do DOM.
//! Os objetos JS dos nós são `Kind::Host { class: NODE, id: NodeId }`.

use std::rc::Rc;

use ferro_js::value::{CallArgs, Kind, NativeFn, ObjRef, Object};
use ferro_js::{Value, Vm};

use super::{Host, exec_script, host, node_of, wrap, wrap_opt};
use crate::dom::{Document, FRAGMENT_TAG, NodeData, NodeId};
use crate::layout::Rect;

type R = Result<Value, Value>;

// ------------------------------------------------------------- utilidades

fn this(vm: &mut Vm, a: &CallArgs) -> Result<NodeId, Value> {
    match &a.this {
        // funções do document chamadas soltas (raro, mas acontece)
        Value::Undefined => Ok(Document::ROOT),
        v => node_of(v).ok_or_else(|| vm.type_error("Illegal invocation")),
    }
}

fn arg_node(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<NodeId, Value> {
    node_of(&a.arg(i)).ok_or_else(|| vm.type_error(format!("parameter {} is not of type 'Node'.", i + 1)))
}

fn arg_string(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<String, Value> {
    vm.to_rust_string(&a.arg(i))
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

fn s(v: &str) -> Value {
    Value::str(v)
}

/// Lança um DOMException (a classe vem do prelúdio).
pub(crate) fn dom_error(vm: &mut Vm, name: &str, msg: &str) -> Value {
    let g = Value::Object(vm.realm.global.clone());
    if let Ok(Value::Object(c)) = vm.get_str(&g, "DOMException")
        && let Ok(v) = vm.construct(&c, vec![s(msg), s(name)], &c)
    {
        return v;
    }
    vm.type_error(msg)
}

