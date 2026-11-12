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

/// Array com o protótipo de NodeList/HTMLCollection.
pub(crate) fn list(vm: &mut Vm, ids: Vec<NodeId>, iface: &str) -> Value {
    let items: Vec<Value> = ids.into_iter().map(|n| wrap(vm, n)).collect();
    let arr = vm.new_array(items);
    if let Some(p) = read(vm, |h| h.protos.get(iface).cloned()) {
        arr.borrow_mut().proto = Some(p);
    }
    Value::Object(arr)
}

fn is_element(doc: &Document, n: NodeId) -> bool {
    doc.element(n).is_some_and(|e| e.tag != FRAGMENT_TAG)
}

fn element_children(doc: &Document, n: NodeId) -> Vec<NodeId> {
    doc.node(n).children.iter().copied().filter(|&c| is_element(doc, c)).collect()
}

fn sibling(doc: &Document, n: NodeId, delta: isize, elements: bool) -> Option<NodeId> {
    let p = doc.node(n).parent?;
    let kids = &doc.node(p).children;
    let mut i = kids.iter().position(|&c| c == n)? as isize;
    loop {
        i += delta;
        if i < 0 || i as usize >= kids.len() {
            return None;
        }
        let c = kids[i as usize];
        if !elements || is_element(doc, c) {
            return Some(c);
        }
    }
}

/// Elemento dentro de `<svg>`?
fn in_svg(doc: &Document, mut n: NodeId) -> bool {
    loop {
        if doc.tag(n) == Some("svg") {
            return true;
        }
        match doc.node(n).parent {
            Some(p) => n = p,
            None => return false,
        }
    }
}

/// Interface (protótipo) de um nó; elementos personalizados trazem o deles.
pub(crate) fn interface_of(h: &Host, n: NodeId) -> (&'static str, Option<ObjRef>) {
    let doc = &h.page.doc;
    match &doc.node(n).data {
        NodeData::Document => ("HTMLDocument", None),
        NodeData::Doctype(_) => ("DocumentType", None),
        NodeData::Text(_) => ("Text", None),
        NodeData::Comment(_) => ("Comment", None),
        NodeData::Element(e) => {
            if e.tag == FRAGMENT_TAG {
                if doc.shadow_hosts.contains_key(&n) {
                    return ("ShadowRoot", None);
                }
                return ("DocumentFragment", None);
            }
            if let Some(c) = h.custom.get(&e.tag)
                && let Some(Value::Object(p)) = c.borrow().own(&ferro_js::value::PropKey::str("prototype")).map(|p| match p.slot {
                    ferro_js::value::Slot::Data { value, .. } => value,
                    _ => Value::Undefined,
                })
            {
                return ("HTMLElement", Some(p));
            }
            if e.tag != "svg" && in_svg(doc, n) {
                return ("SVGElement", None);
            }
            (iface_for_tag(&e.tag), None)
        }
    }
}

