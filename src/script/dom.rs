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

fn iface_for_tag(tag: &str) -> &'static str {
    match tag {
        "a" => "HTMLAnchorElement",
        "area" => "HTMLAreaElement",
        "audio" => "HTMLAudioElement",
        "base" => "HTMLBaseElement",
        "body" => "HTMLBodyElement",
        "br" => "HTMLBRElement",
        "button" => "HTMLButtonElement",
        "canvas" => "HTMLCanvasElement",
        "dialog" => "HTMLDialogElement",
        "div" => "HTMLDivElement",
        "dl" => "HTMLDListElement",
        "form" => "HTMLFormElement",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "HTMLHeadingElement",
        "head" => "HTMLHeadElement",
        "hr" => "HTMLHRElement",
        "html" => "HTMLHtmlElement",
        "iframe" => "HTMLIFrameElement",
        "img" => "HTMLImageElement",
        "input" => "HTMLInputElement",
        "label" => "HTMLLabelElement",
        "li" => "HTMLLIElement",
        "link" => "HTMLLinkElement",
        "meta" => "HTMLMetaElement",
        "ol" => "HTMLOListElement",
        "optgroup" => "HTMLOptGroupElement",
        "option" => "HTMLOptionElement",
        "p" => "HTMLParagraphElement",
        "picture" => "HTMLPictureElement",
        "pre" => "HTMLPreElement",
        "script" => "HTMLScriptElement",
        "select" => "HTMLSelectElement",
        "source" => "HTMLSourceElement",
        "span" => "HTMLSpanElement",
        "style" => "HTMLStyleElement",
        "table" => "HTMLTableElement",
        "tbody" | "thead" | "tfoot" => "HTMLTableSectionElement",
        "td" | "th" => "HTMLTableCellElement",
        "tr" => "HTMLTableRowElement",
        "template" => "HTMLTemplateElement",
        "textarea" => "HTMLTextAreaElement",
        "title" => "HTMLTitleElement",
        "ul" => "HTMLUListElement",
        "video" => "HTMLVideoElement",
        "svg" => "SVGSVGElement",
        t if t.contains('-') => "HTMLElement",
        "abbr" | "address" | "article" | "aside" | "b" | "bdi" | "bdo" | "cite" | "code" | "dd" | "dfn" | "dt"
        | "em" | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "i" | "kbd" | "main" | "mark" | "nav"
        | "noscript" | "rp" | "rt" | "ruby" | "s" | "samp" | "section" | "small" | "strong" | "sub" | "summary"
        | "sup" | "u" | "var" | "wbr" | "center" | "nobr" | "details" => "HTMLElement",
        "font" => "HTMLFontElement",
        "fieldset" => "HTMLFieldSetElement",
        "legend" => "HTMLLegendElement",
        "caption" => "HTMLTableCaptionElement",
        "col" | "colgroup" => "HTMLTableColElement",
        "embed" => "HTMLEmbedElement",
        "object" => "HTMLObjectElement",
        "progress" => "HTMLProgressElement",
        "slot" => "HTMLSlotElement",
        "output" => "HTMLOutputElement",
        "time" => "HTMLTimeElement",
        "data" => "HTMLDataElement",
        "meter" => "HTMLMeterElement",
        "map" => "HTMLMapElement",
        "track" => "HTMLTrackElement",
        "q" | "blockquote" => "HTMLQuoteElement",
        "ins" | "del" => "HTMLModElement",
        "menu" => "HTMLMenuElement",
        _ => "HTMLUnknownElement",
    }
}

/// Interfaces HTML com protótipo próprio (todas herdam de HTMLElement).
const HTML_IFACES: &[&str] = &[
    "HTMLAnchorElement", "HTMLAreaElement", "HTMLMediaElement", "HTMLBaseElement", "HTMLBodyElement",
    "HTMLBRElement", "HTMLButtonElement", "HTMLCanvasElement", "HTMLDialogElement", "HTMLDivElement",
    "HTMLDListElement", "HTMLFormElement", "HTMLHeadingElement", "HTMLHeadElement", "HTMLHRElement",
    "HTMLHtmlElement", "HTMLIFrameElement", "HTMLImageElement", "HTMLInputElement", "HTMLLabelElement",
    "HTMLLIElement", "HTMLLinkElement", "HTMLMetaElement", "HTMLOListElement", "HTMLOptGroupElement",
    "HTMLOptionElement", "HTMLParagraphElement", "HTMLPictureElement", "HTMLPreElement", "HTMLScriptElement",
    "HTMLSelectElement", "HTMLSourceElement", "HTMLSpanElement", "HTMLStyleElement", "HTMLTableElement",
    "HTMLTableSectionElement", "HTMLTableCellElement", "HTMLTableRowElement", "HTMLTemplateElement",
    "HTMLTextAreaElement", "HTMLTitleElement", "HTMLUListElement", "HTMLUnknownElement", "HTMLFontElement",
    "HTMLFieldSetElement", "HTMLLegendElement", "HTMLTableCaptionElement", "HTMLTableColElement",
    "HTMLEmbedElement", "HTMLObjectElement", "HTMLProgressElement", "HTMLSlotElement", "HTMLOutputElement",
    "HTMLTimeElement", "HTMLDataElement", "HTMLMeterElement", "HTMLMapElement", "HTMLTrackElement",
    "HTMLQuoteElement", "HTMLModElement", "HTMLMenuElement",
];

// ---------------------------------------------------- efeitos das mutações

/// Chama um gancho do prelúdio (erros viram "Uncaught ..." no console).
fn call_hook(vm: &mut Vm, name: &str, args: Vec<Value>) {
    let f = read(vm, |h| h.hooks.get(name).cloned());
    if let Some(f) = f
        && let Err(e) = vm.call(&f, Value::Undefined, args)
    {
        vm.report_uncaught(&e);
    }
}

/// Avisa os MutationObserver (só se algum estiver ativo).
fn notify(vm: &mut Vm, kind: &str, target: NodeId, added: &[NodeId], removed: &[NodeId], attr: Option<&str>, old: Option<String>) {
    if !read(vm, |h| h.observing) {
        return;
    }
    let target = wrap(vm, target);
    let added: Vec<Value> = added.iter().map(|&n| wrap(vm, n)).collect();
    let removed: Vec<Value> = removed.iter().map(|&n| wrap(vm, n)).collect();
    let added = vm.array_value(added);
    let removed = vm.array_value(removed);
    let attr = attr.map_or(Value::Null, s);
    let old = old.map_or(Value::Null, |o| s(&o));
    call_hook(vm, "mutation", vec![s(kind), target, added, removed, attr, old]);
}

/// Nó inerte: dentro do conteúdo de um `<template>` ou de outro documento.
fn is_inert(h: &Host, n: NodeId) -> bool {
    let doc = &h.page.doc;
    let mut r = n;
    while let Some(p) = doc.node(r).parent {
        r = p;
    }
    (matches!(doc.node(r).data, NodeData::Document) && r != Document::ROOT) || h.templates.values().any(|&t| t == r)
}

/// Roda o construtor dos elementos personalizados ainda não promovidos.
fn try_upgrade(vm: &mut Vm, nodes: &[NodeId]) {
    let list: Vec<NodeId> = custom_in(vm, nodes)
        .into_iter()
        .filter(|&n| read(vm, |h| !h.upgraded.contains(&n) && !is_inert(h, n)))
        .collect();
    for n in list {
        let el = wrap(vm, n);
        if let Err(e) = upgrade_custom(vm, n, &el) {
            vm.report_uncaught(&e);
        }
    }
}

/// Elementos personalizados dentro de `nodes` (inclusive).
fn custom_in(vm: &Vm, nodes: &[NodeId]) -> Vec<NodeId> {
    read(vm, |h| {
        if h.custom.is_empty() {
            return Vec::new();
        }
        let doc = &h.page.doc;
        let mut out = Vec::new();
        for &n in nodes {
            let mut all = vec![n];
            all.extend(doc.descendants(n));
            for d in all {
                if doc.tag(d).is_some_and(|t| h.custom.contains_key(t)) {
                    out.push(d);
                }
            }
        }
        out
    })
}

/// Depois de inserir nós: marca o layout como sujo, recoleta estilos e roda
/// os `<script>` que acabaram de entrar no documento.
pub(crate) fn inserted(vm: &mut Vm, nodes: &[NodeId], parent: NodeId) {
    let mut scripts = Vec::new();
    write(vm, |h| {
        h.dirty = true;
        let doc = &h.page.doc;
        if !doc.is_connected(parent) {
            return;
        }
        match doc.tag(parent) {
            Some("style") => h.styles_dirty = true,
            Some("script") if !h.started.contains(&parent) => scripts.push(parent),
            Some("title") => h.title_changed = true,
            _ => {}
        }
        for &n in nodes {
            let mut all = vec![n];
            all.extend(doc.descendants(n));
            for d in all {
                match doc.tag(d) {
                    Some("style" | "link") => h.styles_dirty = true,
                    Some("script") if !h.started.contains(&d) => scripts.push(d),
                    Some("title") => h.title_changed = true,
                    _ => {}
                }
            }
        }
    });
    notify(vm, "childList", parent, nodes, &[], None, None);
    // inserir no documento promove os elementos personalizados (mesmo fora da árvore)
    try_upgrade(vm, nodes);
    if read(vm, |h| h.page.doc.is_connected(parent)) {
        for n in custom_in(vm, nodes) {
            let el = wrap(vm, n);
            call_hook(vm, "connected", vec![el]);
        }
    }
    for n in scripts {
        start_script(vm, n);
    }
}

fn removing(vm: &mut Vm, n: NodeId) {
    if let Some(p) = read(vm, |h| h.page.doc.node(n).parent) {
        notify(vm, "childList", p, &[], &[n], None, None);
        if read(vm, |h| h.page.doc.is_connected(n)) {
            for c in custom_in(vm, &[n]) {
                let el = wrap(vm, c);
                call_hook(vm, "disconnected", vec![el]);
            }
        }
    }
    write(vm, |h| {
        h.dirty = true;
        let doc = &h.page.doc;
        if !doc.is_connected(n) {
            return;
        }
        if matches!(doc.tag(n), Some("style" | "link"))
            || doc.descendants(n).iter().any(|&d| matches!(doc.tag(d), Some("style" | "link")))
            || doc.node(n).parent.is_some_and(|p| doc.tag(p) == Some("style"))
        {
            h.styles_dirty = true;
        }
    });
}

/// Um `<script>` entrou no documento (ou ganhou `src`/texto): roda ou baixa.
pub(crate) fn start_script(vm: &mut Vm, n: NodeId) {
    let h = host(vm);
    let (classic, special, src, text) = {
        let hb = h.borrow();
        let doc = &hb.page.doc;
        if hb.started.contains(&n) || !doc.is_connected(n) {
            return;
        }
        let src = doc.element(n).and_then(|e| e.attr("src")).map(str::to_string);
        (super::is_classic_script(doc, n), super::script_type(doc, n), src, doc.text_content(n))
    };
    // módulo ou import map inseridos por script
    if let Some(ty) = special {
        if src.is_none() && text.trim().is_empty() {
            return;
        }
        h.borrow_mut().started.insert(n);
        // os do HTML são tratados pelo parser
        if h.borrow().parser.is_some() {
            return;
        }
        if ty == "module" {
            super::module::start_script(vm, n, src, &text);
            super::module::drive(vm);
        } else if src.is_none() {
            super::module::add_import_map(vm, &text);
        }
        return;
    }
    match src {
        Some(src) => {
            let mut hb = h.borrow_mut();
            hb.started.insert(n);
            if !classic {
                return;
            }
            let Ok(url) = hb.page.base.join(src.trim()) else { return };
            let net = hb.net.clone();
            hb.in_flight += 1;
            hb.pool.submit(Box::new(move || {
                let text = match net.fetch(&url) {
                    Ok(r) if r.status < 400 => Some(crate::net::decode_text(&r.body, r.content_type.as_deref(), false)),
                    _ => None,
                };
                super::Done::Script { node: n, text }
            }));
        }
        None => {
            // sem texto ainda: roda quando o texto chegar
            if text.trim().is_empty() {
                return;
            }
            h.borrow_mut().started.insert(n);
            if classic {
                let name = format!("{}#inline", h.borrow().page.url);
                exec_script(vm, n, &text, &name);
            }
        }
    }
}

/// Atributo mudou: efeitos colaterais (folhas, scripts, imagens, observers).
fn attr_changed(vm: &mut Vm, n: NodeId, name: &str, old: Option<String>) {
    notify(vm, "attributes", n, &[], &[], Some(name), old.clone());
    if read(vm, |h| h.upgraded.contains(&n)) {
        let el = wrap(vm, n);
        let new = attr_of(vm, n, name).map_or(Value::Null, |v| s(&v));
        call_hook(vm, "attributeChanged", vec![el, s(name), old.map_or(Value::Null, |o| s(&o)), new]);
    }
    let (tag, connected) = write(vm, |h| {
        h.dirty = true;
        (h.page.doc.tag(n).unwrap_or("").to_string(), h.page.doc.is_connected(n))
    });
    match (tag.as_str(), name) {
        ("link", "href" | "rel" | "media" | "disabled") | ("style", "media") if connected => {
            write(vm, |h| h.styles_dirty = true)
        }
        ("script", "src") if connected => start_script(vm, n),
        ("canvas", "width" | "height") => write(vm, |h| super::canvas::attr_resized(h, n)),
        ("img", "src") => {
            write(vm, |h| {
                let src = h.page.doc.element(n).and_then(|e| e.attr("src")).map(str::to_string);
                if let Some(u) = src.and_then(|s| h.page.base.join(s.trim()).ok()) {
                    super::request_image(h, u, Some(n));
                }
            });
        }
        _ => {}
    }
}

/// Insere `child` em `parent` antes de `before`, com as validações do DOM.
fn insert(vm: &mut Vm, parent: NodeId, child: NodeId, before: Option<NodeId>) -> Result<(), Value> {
    let (bad, ref_ok, nodes, valid_parent) = read(vm, |h| {
        let doc = &h.page.doc;
        let nodes = if doc.is_fragment(child) { doc.node(child).children.clone() } else { vec![child] };
        (
            doc.is_inclusive_ancestor(child, parent),
            before.is_none_or(|b| doc.node(b).parent == Some(parent)),
            nodes,
            matches!(doc.node(parent).data, NodeData::Element(_) | NodeData::Document),
        )
    });
    if bad || !valid_parent || child == Document::ROOT {
        return Err(dom_error(vm, "HierarchyRequestError", "The new child element contains the parent."));
    }
    if !ref_ok {
        return Err(dom_error(
            vm,
            "NotFoundError",
            "The node before which the new node is to be inserted is not a child of this node.",
        ));
    }
    let was_connected = read(vm, |h| h.page.doc.is_connected(child));
    if was_connected {
        removing(vm, child);
    }
    write(vm, |h| h.page.doc.insert_before(parent, child, before));
    inserted(vm, &nodes, parent);
    Ok(())
}

/// Argumentos de append/prepend/before/after: nós ou textos (viram nós de texto).
fn nodes_from_args(vm: &mut Vm, a: &CallArgs) -> Result<Vec<NodeId>, Value> {
    let mut out = Vec::new();
    for v in &a.args {
        match node_of(v) {
            Some(n) => out.push(n),
            None => {
                let text = vm.to_rust_string(v)?;
                out.push(write(vm, |h| h.page.doc.create(NodeData::Text(text))));
            }
        }
    }
    Ok(out)
}

fn insert_all(vm: &mut Vm, parent: NodeId, nodes: Vec<NodeId>, before: Option<NodeId>) -> Result<(), Value> {
    for n in nodes {
        insert(vm, parent, n, before)?;
    }
    Ok(())
}

// ------------------------------------------------------------------- Node

fn node_type(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Number(read(vm, |h| match &h.page.doc.node(n).data {
        NodeData::Element(e) if e.tag == FRAGMENT_TAG => 11.0,
        NodeData::Element(_) => 1.0,
        NodeData::Text(_) => 3.0,
        NodeData::Comment(_) => 8.0,
        NodeData::Document => 9.0,
        NodeData::Doctype(_) => 10.0,
    })))
}

fn node_name(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(s(&read(vm, |h| match &h.page.doc.node(n).data {
        NodeData::Element(e) if e.tag == FRAGMENT_TAG => "#document-fragment".to_string(),
        NodeData::Element(e) => {
            if in_svg(&h.page.doc, n) { e.tag.clone() } else { e.tag.to_ascii_uppercase() }
        }
        NodeData::Text(_) => "#text".into(),
        NodeData::Comment(_) => "#comment".into(),
        NodeData::Document => "#document".into(),
        NodeData::Doctype(d) => d.split_whitespace().next().unwrap_or("html").to_string(),
    })))
}

fn char_data(vm: &mut Vm, n: NodeId) -> Option<String> {
    read(vm, |h| match &h.page.doc.node(n).data {
        NodeData::Text(t) | NodeData::Comment(t) => Some(t.clone()),
        _ => None,
    })
}

fn node_value(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(char_data(vm, n).map_or(Value::Null, |t| s(&t)))
}

fn set_node_value(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    if char_data(vm, n).is_some() {
        set_data_of(vm, n, &a.arg(0))?;
    }
    Ok(Value::Undefined)
}

fn set_data_of(vm: &mut Vm, n: NodeId, v: &Value) -> Result<(), Value> {
    let text = if v.is_nullish() { String::new() } else { vm.to_rust_string(v)? };
    let old = char_data(vm, n);
    notify(vm, "characterData", n, &[], &[], None, old);
    let parent = write(vm, |h| {
        h.page.doc.set_data(n, &text);
        h.dirty = true;
        h.page.doc.node(n).parent
    });
    if let Some(p) = parent {
        inserted(vm, &[], p);
    }
    Ok(())
}

fn text_content(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(read(vm, |h| match &h.page.doc.node(n).data {
        NodeData::Document | NodeData::Doctype(_) => Value::Null,
        NodeData::Text(t) | NodeData::Comment(t) => s(t),
        NodeData::Element(_) => s(&h.page.doc.text_content(n)),
    }))
}

fn set_text_content(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let v = a.arg(0);
    let text = if v.is_nullish() { String::new() } else { vm.to_rust_string(&v)? };
    if char_data(vm, n).is_some() {
        set_data_of(vm, n, &a.arg(0))?;
        return Ok(Value::Undefined);
    }
    if n == Document::ROOT {
        return Ok(Value::Undefined);
    }
    removing_children(vm, n);
    let kids = write(vm, |h| {
        h.page.doc.set_text_content(n, &text);
        h.page.doc.node(n).children.clone()
    });
    inserted(vm, &kids, n);
    Ok(Value::Undefined)
}

fn removing_children(vm: &mut Vm, n: NodeId) {
    let kids = read(vm, |h| h.page.doc.node(n).children.clone());
    for k in kids {
        removing(vm, k);
    }
}

fn parent_node(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let p = read(vm, |h| h.page.doc.node(n).parent);
    Ok(wrap_opt(vm, p))
}

fn parent_element(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let p = read(vm, |h| h.page.doc.node(n).parent.filter(|&p| is_element(&h.page.doc, p)));
    Ok(wrap_opt(vm, p))
}

fn child_nodes(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let kids = read(vm, |h| h.page.doc.node(n).children.clone());
    Ok(list(vm, kids, "NodeList"))
}

fn first_child(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let c = read(vm, |h| h.page.doc.node(n).children.first().copied());
    Ok(wrap_opt(vm, c))
}

fn last_child(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let c = read(vm, |h| h.page.doc.node(n).children.last().copied());
    Ok(wrap_opt(vm, c))
}

fn previous_sibling(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let c = read(vm, |h| sibling(&h.page.doc, n, -1, false));
    Ok(wrap_opt(vm, c))
}

fn next_sibling(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let c = read(vm, |h| sibling(&h.page.doc, n, 1, false));
    Ok(wrap_opt(vm, c))
}

fn owner_document(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let root = read(vm, |h| {
        let doc = &h.page.doc;
        if matches!(doc.node(n).data, NodeData::Document) {
            return None;
        }
        let mut r = n;
        while let Some(p) = doc.node(r).parent {
            r = p;
        }
        Some(if matches!(doc.node(r).data, NodeData::Document) { r } else { Document::ROOT })
    });
    Ok(match root {
        Some(r) => wrap(vm, r),
        None => Value::Null,
    })
}

fn is_connected(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Bool(read(vm, |h| h.page.doc.is_connected(n))))
}

fn base_uri(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(s(read(vm, |h| h.page.base.to_string()).as_str()))
}

fn append_child(vm: &mut Vm, a: &mut CallArgs) -> R {
    let p = this(vm, a)?;
    let c = arg_node(vm, a, 0)?;
    insert(vm, p, c, None)?;
    Ok(a.arg(0))
}

fn insert_before(vm: &mut Vm, a: &mut CallArgs) -> R {
    let p = this(vm, a)?;
    let c = arg_node(vm, a, 0)?;
    let before = node_of(&a.arg(1));
    insert(vm, p, c, before)?;
    Ok(a.arg(0))
}

fn remove_child(vm: &mut Vm, a: &mut CallArgs) -> R {
    let p = this(vm, a)?;
    let c = arg_node(vm, a, 0)?;
    if read(vm, |h| h.page.doc.node(c).parent) != Some(p) {
        return Err(dom_error(vm, "NotFoundError", "The node to be removed is not a child of this node."));
    }
    removing(vm, c);
    write(vm, |h| h.page.doc.detach(c));
    Ok(a.arg(0))
}

fn replace_child(vm: &mut Vm, a: &mut CallArgs) -> R {
    let p = this(vm, a)?;
    let new = arg_node(vm, a, 0)?;
    let old = arg_node(vm, a, 1)?;
    if read(vm, |h| h.page.doc.node(old).parent) != Some(p) {
        return Err(dom_error(vm, "NotFoundError", "The node to be replaced is not a child of this node."));
    }
    if new != old {
        let next = read(vm, |h| sibling(&h.page.doc, old, 1, false));
        removing(vm, old);
        write(vm, |h| h.page.doc.detach(old));
        let next = next.filter(|&x| x != new);
        insert(vm, p, new, next)?;
    }
    Ok(a.arg(1))
}

fn clone_node(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let deep = ferro_js::vm::truthy(&a.arg(0));
    let c = write(vm, |h| {
        let c = h.page.doc.clone_node(n, deep);
        // scripts copiados não rodam de novo
        let mut all = vec![c];
        all.extend(h.page.doc.descendants(c));
        let orig = {
            let mut o = vec![n];
            o.extend(h.page.doc.descendants(n));
            o
        };
        for (x, y) in orig.iter().zip(all.iter()) {
            if h.started.contains(x) {
                h.started.insert(*y);
            }
        }
        c
    });
    try_upgrade(vm, &[c]);
    Ok(wrap(vm, c))
}

fn contains(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let Some(o) = node_of(&a.arg(0)) else { return Ok(Value::Bool(false)) };
    Ok(Value::Bool(read(vm, |h| h.page.doc.is_inclusive_ancestor(n, o))))
}

fn has_child_nodes(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Bool(read(vm, |h| !h.page.doc.node(n).children.is_empty())))
}

fn get_root_node(vm: &mut Vm, a: &mut CallArgs) -> R {
    let mut n = this(vm, a)?;
    while let Some(p) = read(vm, |h| h.page.doc.node(n).parent) {
        n = p;
    }
    Ok(wrap(vm, n))
}

fn is_same_node(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Bool(node_of(&a.arg(0)) == Some(n)))
}

fn is_equal_node(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let Some(o) = node_of(&a.arg(0)) else { return Ok(Value::Bool(false)) };
    Ok(Value::Bool(read(vm, |h| {
        let d = &h.page.doc;
        d.serialize(n, true) == d.serialize(o, true)
    })))
}

/// Posição em pré-ordem (para compareDocumentPosition).
fn path_to_root(doc: &Document, mut n: NodeId) -> Vec<NodeId> {
    let mut p = vec![n];
    while let Some(x) = doc.node(n).parent {
        p.push(x);
        n = x;
    }
    p.reverse();
    p
}

fn compare_document_position(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let o = arg_node(vm, a, 0)?;
    if n == o {
        return Ok(Value::Number(0.0));
    }
    let r = read(vm, |h| {
        let doc = &h.page.doc;
        let (pa, pb) = (path_to_root(doc, n), path_to_root(doc, o));
        if pa[0] != pb[0] {
            return 1 | 32 | if o < n { 2 } else { 4 };
        }
        let mut i = 0;
        while i < pa.len() && i < pb.len() && pa[i] == pb[i] {
            i += 1;
        }
        if i == pa.len() {
            // o é descendente de n
            return 16 | 4;
        }
        if i == pb.len() {
            return 8 | 2;
        }
        let kids = &doc.node(pa[i - 1]).children;
        let ia = kids.iter().position(|&c| c == pa[i]);
        let ib = kids.iter().position(|&c| c == pb[i]);
        if ib < ia { 2 } else { 4 }
    });
    Ok(Value::Number(r as f64))
}

fn normalize(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    write(vm, |h| {
        let doc = &mut h.page.doc;
        let mut stack = vec![n];
        while let Some(p) = stack.pop() {
            let kids = doc.node(p).children.clone();
            let mut prev_text: Option<NodeId> = None;
            for k in kids {
                let text = match &doc.node(k).data {
                    NodeData::Text(t) => Some(t.clone()),
                    _ => None,
                };
                match text {
                    Some(t) if t.is_empty() => doc.detach(k),
                    Some(t) => match prev_text {
                        Some(pt) => {
                            let mut joined = match &doc.node(pt).data {
                                NodeData::Text(x) => x.clone(),
                                _ => String::new(),
                            };
                            joined.push_str(&t);
                            doc.set_data(pt, &joined);
                            doc.detach(k);
                        }
                        None => prev_text = Some(k),
                    },
                    None => {
                        prev_text = None;
                        stack.push(k);
                    }
                }
            }
        }
        h.dirty = true;
    });
    Ok(Value::Undefined)
}

// --------------------------------------------------------- CharacterData

fn cd_data(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(s(&char_data(vm, n).unwrap_or_default()))
}

fn cd_set_data(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    set_data_of(vm, n, &a.arg(0))?;
    Ok(Value::Undefined)
}

fn cd_length(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let t = char_data(vm, n).unwrap_or_default();
    Ok(Value::Number(t.encode_utf16().count() as f64))
}

fn cd_append_data(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let mut t = char_data(vm, n).unwrap_or_default();
    t.push_str(&arg_string(vm, a, 0)?);
    set_data_of(vm, n, &s(&t))?;
    Ok(Value::Undefined)
}

// ------------------------------------------------------------- ParentNode

fn children(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let kids = read(vm, |h| element_children(&h.page.doc, n));
    Ok(list(vm, kids, "HTMLCollection"))
}

fn first_element_child(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let c = read(vm, |h| element_children(&h.page.doc, n).first().copied());
    Ok(wrap_opt(vm, c))
}

fn last_element_child(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let c = read(vm, |h| element_children(&h.page.doc, n).last().copied());
    Ok(wrap_opt(vm, c))
}

fn child_element_count(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Number(read(vm, |h| element_children(&h.page.doc, n).len()) as f64))
}

fn previous_element_sibling(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let c = read(vm, |h| sibling(&h.page.doc, n, -1, true));
    Ok(wrap_opt(vm, c))
}

fn next_element_sibling(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let c = read(vm, |h| sibling(&h.page.doc, n, 1, true));
    Ok(wrap_opt(vm, c))
}

fn append(vm: &mut Vm, a: &mut CallArgs) -> R {
    let p = this(vm, a)?;
    let nodes = nodes_from_args(vm, a)?;
    insert_all(vm, p, nodes, None)?;
    Ok(Value::Undefined)
}

fn prepend(vm: &mut Vm, a: &mut CallArgs) -> R {
    let p = this(vm, a)?;
    let nodes = nodes_from_args(vm, a)?;
    let first = read(vm, |h| h.page.doc.node(p).children.first().copied());
    let first = first.filter(|f| !nodes.contains(f));
    insert_all(vm, p, nodes, first)?;
    Ok(Value::Undefined)
}

fn replace_children(vm: &mut Vm, a: &mut CallArgs) -> R {
    let p = this(vm, a)?;
    let nodes = nodes_from_args(vm, a)?;
    removing_children(vm, p);
    write(vm, |h| {
        let kids = h.page.doc.node(p).children.clone();
        for k in kids {
            if !nodes.contains(&k) {
                h.page.doc.detach(k);
            }
        }
    });
    insert_all(vm, p, nodes, None)?;
    Ok(Value::Undefined)
}

// -------------------------------------------------------------- ChildNode

fn before(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let nodes = nodes_from_args(vm, a)?;
    let Some(p) = read(vm, |h| h.page.doc.node(n).parent) else { return Ok(Value::Undefined) };
    insert_all(vm, p, nodes, Some(n))?;
    Ok(Value::Undefined)
}

fn after(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let nodes = nodes_from_args(vm, a)?;
    let Some(p) = read(vm, |h| h.page.doc.node(n).parent) else { return Ok(Value::Undefined) };
    // o próximo irmão que não está sendo movido
    let next = read(vm, |h| {
        let mut x = sibling(&h.page.doc, n, 1, false);
        while let Some(c) = x {
            if !nodes.contains(&c) {
                break;
            }
            x = sibling(&h.page.doc, c, 1, false);
        }
        x
    });
    insert_all(vm, p, nodes, next)?;
    Ok(Value::Undefined)
}

fn replace_with(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let nodes = nodes_from_args(vm, a)?;
    let Some(p) = read(vm, |h| h.page.doc.node(n).parent) else { return Ok(Value::Undefined) };
    let next = read(vm, |h| {
        let mut x = sibling(&h.page.doc, n, 1, false);
        while let Some(c) = x {
            if !nodes.contains(&c) {
                break;
            }
            x = sibling(&h.page.doc, c, 1, false);
        }
        x
    });
    if !nodes.contains(&n) {
        removing(vm, n);
        write(vm, |h| h.page.doc.detach(n));
    }
    insert_all(vm, p, nodes, next)?;
    Ok(Value::Undefined)
}

fn remove(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    removing(vm, n);
    write(vm, |h| h.page.doc.detach(n));
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------- Element

fn tag_name(vm: &mut Vm, a: &mut CallArgs) -> R {
    node_name(vm, a)
}

fn local_name(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(read(vm, |h| h.page.doc.tag(n).map_or(Value::Null, s)))
}

fn namespace_uri(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(s(if read(vm, |h| in_svg(&h.page.doc, n)) {
        "http://www.w3.org/2000/svg"
    } else {
        "http://www.w3.org/1999/xhtml"
    }))
}

fn attr_of(vm: &Vm, n: NodeId, name: &str) -> Option<String> {
    read(vm, |h| h.page.doc.element(n).and_then(|e| e.attr(name)).map(str::to_string))
}

/// Nomes de atributo HTML são insensíveis a maiúsculas.
fn attr_name(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<String, Value> {
    Ok(vm.to_rust_string(&a.arg(i))?.to_ascii_lowercase())
}

fn get_attribute(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let name = attr_name(vm, a, 0)?;
    Ok(attr_of(vm, n, &name).map_or(Value::Null, |v| s(&v)))
}

fn set_attr_checked(vm: &mut Vm, n: NodeId, name: &str, value: &str) -> Result<(), Value> {
    if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '>' || c == '/' || c == '=') {
        return Err(dom_error(vm, "InvalidCharacterError", &format!("'{name}' is not a valid attribute name.")));
    }
    let old = attr_of(vm, n, name);
    write(vm, |h| h.page.doc.set_attr(n, name, value));
    attr_changed(vm, n, name, old);
    Ok(())
}

fn set_attribute(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let name = attr_name(vm, a, 0)?;
    let value = arg_string(vm, a, 1)?;
    set_attr_checked(vm, n, &name, &value)?;
    Ok(Value::Undefined)
}

fn remove_attribute(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let name = attr_name(vm, a, 0)?;
    let old = attr_of(vm, n, &name);
    if write(vm, |h| h.page.doc.remove_attr(n, &name)) {
        attr_changed(vm, n, &name, old);
    }
    Ok(Value::Undefined)
}

fn has_attribute(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let name = attr_name(vm, a, 0)?;
    Ok(Value::Bool(attr_of(vm, n, &name).is_some()))
}

fn has_attributes(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Bool(read(vm, |h| h.page.doc.element(n).is_some_and(|e| !e.attrs.is_empty()))))
}

fn toggle_attribute(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let name = attr_name(vm, a, 0)?;
    let has = attr_of(vm, n, &name).is_some();
    let want = if a.args.len() > 1 { ferro_js::vm::truthy(&a.arg(1)) } else { !has };
    if want && !has {
        set_attr_checked(vm, n, &name, "")?;
    } else if !want && has {
        let old = attr_of(vm, n, &name);
        write(vm, |h| h.page.doc.remove_attr(n, &name));
        attr_changed(vm, n, &name, old);
    }
    Ok(Value::Bool(want))
}

fn get_attribute_names(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let names: Vec<Value> = read(vm, |h| {
        h.page.doc.element(n).map(|e| e.attrs.iter().map(|(k, _)| s(k)).collect()).unwrap_or_default()
    });
    Ok(vm.array_value(names))
}

fn attributes(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let attrs = read(vm, |h| h.page.doc.element(n).map(|e| e.attrs.clone()).unwrap_or_default());
    let mut items = Vec::new();
    for (k, v) in attrs {
        let o = vm.new_object();
        for (key, val) in [("name", &k), ("localName", &k), ("nodeName", &k), ("value", &v), ("nodeValue", &v)] {
            vm.define_data(&o, ferro_js::value::PropKey::str(key), s(val));
        }
        vm.define_data(&o, ferro_js::value::PropKey::str("specified"), Value::Bool(true));
        items.push(Value::Object(o));
    }
    let arr = vm.new_array(items);
    if let Some(p) = read(vm, |h| h.protos.get("NamedNodeMap").cloned()) {
        arr.borrow_mut().proto = Some(p);
    }
    Ok(Value::Object(arr))
}

fn id(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(s(&attr_of(vm, n, "id").unwrap_or_default()))
}

fn set_id(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let v = arg_string(vm, a, 0)?;
    set_attr_checked(vm, n, "id", &v)?;
    Ok(Value::Undefined)
}

fn class_name(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(s(&attr_of(vm, n, "class").unwrap_or_default()))
}

fn set_class_name(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let v = arg_string(vm, a, 0)?;
    set_attr_checked(vm, n, "class", &v)?;
    Ok(Value::Undefined)
}

/// Contexto de parsing do innerHTML (tabelas e listas mudam as regras).
fn fragment_context(doc: &Document, n: NodeId) -> String {
    doc.tag(n).filter(|t| *t != FRAGMENT_TAG).unwrap_or("body").to_string()
}

/// Analisa HTML como filhos de `ctx`; os `<script>` nunca rodam.
fn parse_html(vm: &mut Vm, html: &str, ctx: NodeId) -> Vec<NodeId> {
    write(vm, |h| {
        let context = fragment_context(&h.page.doc, ctx);
        let nodes = crate::html::parse_fragment(html, &context, &mut h.page.doc);
        for &n in &nodes {
            let mut all = vec![n];
            all.extend(h.page.doc.descendants(n));
            for d in all {
                if h.page.doc.tag(d) == Some("script") {
                    h.started.insert(d);
                }
            }
        }
        nodes
    })
}

fn inner_html(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let n = read(vm, |h| h.templates.get(&n).copied().unwrap_or(n));
    Ok(s(&read(vm, |h| h.page.doc.serialize(n, false))))
}

fn set_inner_html(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let html = if a.arg(0).is_nullish() { String::new() } else { arg_string(vm, a, 0)? };
    let target = read(vm, |h| h.templates.get(&n).copied().unwrap_or(n));
    removing_children(vm, target);
    write(vm, |h| h.page.doc.remove_children(target));
    let nodes = parse_html(vm, &html, n);
    write(vm, |h| {
        for &c in &nodes {
            h.page.doc.append(target, c);
        }
    });
    inserted(vm, &nodes, target);
    Ok(Value::Undefined)
}

fn outer_html(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(s(&read(vm, |h| h.page.doc.serialize(n, true))))
}

fn set_outer_html(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let html = arg_string(vm, a, 0)?;
    let Some(p) = read(vm, |h| h.page.doc.node(n).parent) else { return Ok(Value::Undefined) };
    let nodes = parse_html(vm, &html, p);
    removing(vm, n);
    write(vm, |h| {
        for &c in &nodes {
            h.page.doc.insert_before(p, c, Some(n));
        }
        h.page.doc.detach(n);
    });
    inserted(vm, &nodes, p);
    Ok(Value::Undefined)
}

/// Onde inserir para insertAdjacent*: (pai, antes de).
fn adjacent(vm: &mut Vm, n: NodeId, pos: &str) -> Result<Option<(NodeId, Option<NodeId>)>, Value> {
    let pos = pos.to_ascii_lowercase();
    Ok(read(vm, |h| {
        let doc = &h.page.doc;
        match pos.as_str() {
            "beforebegin" => doc.node(n).parent.map(|p| (p, Some(n))),
            "afterbegin" => Some((n, doc.node(n).children.first().copied())),
            "beforeend" => Some((n, None)),
            "afterend" => doc.node(n).parent.map(|p| (p, sibling(doc, n, 1, false))),
            _ => None,
        }
    }))
    .and_then(|r| match r {
        Some(x) => Ok(Some(x)),
        None if !matches!(pos.as_str(), "beforebegin" | "afterbegin" | "beforeend" | "afterend") => Err(dom_error(
            vm,
            "SyntaxError",
            &format!("The value provided ('{pos}') is not one of 'beforeBegin', 'afterBegin', 'beforeEnd', or 'afterEnd'."),
        )),
        None => Ok(None),
    })
}

fn insert_adjacent_html(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let pos = arg_string(vm, a, 0)?;
    let html = arg_string(vm, a, 1)?;
    let Some((p, before)) = adjacent(vm, n, &pos)? else { return Ok(Value::Undefined) };
    let nodes = parse_html(vm, &html, p);
    write(vm, |h| {
        for &c in &nodes {
            h.page.doc.insert_before(p, c, before);
        }
    });
    inserted(vm, &nodes, p);
    Ok(Value::Undefined)
}

fn insert_adjacent_element(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let pos = arg_string(vm, a, 0)?;
    let el = arg_node(vm, a, 1)?;
    let Some((p, before)) = adjacent(vm, n, &pos)? else { return Ok(Value::Null) };
    insert(vm, p, el, before)?;
    Ok(a.arg(1))
}

fn insert_adjacent_text(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let pos = arg_string(vm, a, 0)?;
    let text = arg_string(vm, a, 1)?;
    let Some((p, before)) = adjacent(vm, n, &pos)? else { return Ok(Value::Undefined) };
    let t = write(vm, |h| h.page.doc.create(NodeData::Text(text)));
    insert(vm, p, t, before)?;
    Ok(Value::Undefined)
}

// -------------------------------------------------------------- seletores

/// Compila um seletor; `:scope` vira um atributo temporário no elemento base.
fn selector(vm: &mut Vm, src: &str) -> Result<(Vec<crate::css::Selector>, bool), Value> {
    let scoped = src.contains(":scope");
    let text = src.replace(":scope", "[data-ferro-scope]");
    match crate::css::parse_selector_list(&text) {
        Some(s) => Ok((s, scoped)),
        None => Err(dom_error(vm, "SyntaxError", &format!("'{src}' is not a valid selector."))),
    }
}

fn select(vm: &mut Vm, root: NodeId, src: &str, first: bool) -> Result<Vec<NodeId>, Value> {
    let (sels, scoped) = selector(vm, src)?;
    Ok(write(vm, |h| {
        if scoped {
            h.page.doc.set_attr(root, "data-ferro-scope", "");
        }
        let doc = &h.page.doc;
        let mut out = Vec::new();
        for d in doc.descendants(root) {
            if is_element(doc, d) && sels.iter().any(|s| crate::css::matches(doc, d, s)) {
                out.push(d);
                if first {
                    break;
                }
            }
        }
        if scoped {
            h.page.doc.remove_attr(root, "data-ferro-scope");
        }
        out
    }))
}

fn query_selector(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let src = arg_string(vm, a, 0)?;
    let found = select(vm, n, &src, true)?;
    Ok(wrap_opt(vm, found.first().copied()))
}

fn query_selector_all(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let src = arg_string(vm, a, 0)?;
    let found = select(vm, n, &src, false)?;
    Ok(list(vm, found, "NodeList"))
}

fn matches_sel(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let src = arg_string(vm, a, 0)?;
    let (sels, _) = selector(vm, &src)?;
    Ok(Value::Bool(read(vm, |h| sels.iter().any(|s| crate::css::matches(&h.page.doc, n, s)))))
}

fn closest(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let src = arg_string(vm, a, 0)?;
    let (sels, _) = selector(vm, &src)?;
    let found = read(vm, |h| {
        let doc = &h.page.doc;
        let mut x = Some(n);
        while let Some(c) = x {
            if is_element(doc, c) && sels.iter().any(|s| crate::css::matches(doc, c, s)) {
                return Some(c);
            }
            x = doc.node(c).parent;
        }
        None
    });
    Ok(wrap_opt(vm, found))
}

fn get_elements_by_tag_name(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let tag = arg_string(vm, a, 0)?.to_ascii_lowercase();
    let found = read(vm, |h| {
        let doc = &h.page.doc;
        doc.descendants(n)
            .into_iter()
            .filter(|&d| is_element(doc, d) && (tag == "*" || doc.tag(d) == Some(tag.as_str())))
            .collect()
    });
    Ok(list(vm, found, "HTMLCollection"))
}

fn get_elements_by_class_name(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let names = arg_string(vm, a, 0)?;
    let wanted: Vec<&str> = names.split_ascii_whitespace().collect();
    let found = read(vm, |h| {
        let doc = &h.page.doc;
        if wanted.is_empty() {
            return Vec::new();
        }
        doc.descendants(n)
            .into_iter()
            .filter(|&d| doc.element(d).is_some_and(|e| wanted.iter().all(|w| e.has_class(w))))
            .collect()
    });
    Ok(list(vm, found, "HTMLCollection"))
}

fn get_element_by_id(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let want = arg_string(vm, a, 0)?;
    let found = read(vm, |h| {
        let doc = &h.page.doc;
        doc.descendants(n).into_iter().find(|&d| doc.element(d).and_then(|e| e.id()) == Some(want.as_str()))
    });
    Ok(wrap_opt(vm, found))
}

fn get_elements_by_name(vm: &mut Vm, a: &mut CallArgs) -> R {
    let want = arg_string(vm, a, 0)?;
    let found = read(vm, |h| {
        let doc = &h.page.doc;
        doc.descendants(Document::ROOT)
            .into_iter()
            .filter(|&d| doc.element(d).and_then(|e| e.attr("name")) == Some(want.as_str()))
            .collect()
    });
    Ok(list(vm, found, "NodeList"))
}

// ----------------------------------------------------------------- layout

/// Retângulo (coordenadas do documento) que envolve as caixas do elemento.
fn rect_of(vm: &mut Vm, n: NodeId) -> Option<Rect> {
    let h = host(vm);
    super::ensure_layout(&h);
    let hb = h.borrow();
    let info = hb.layout.as_ref()?;
    // dentro de `position: fixed`, a caixa está na janela: soma a rolagem
    let shift = if info.fixed.contains(&n) { hb.scroll_y } else { 0.0 };
    let mut out: Option<Rect> = None;
    for (id, r) in &info.boxes {
        let r = &Rect { y: r.y + shift, ..*r };
        if *id != n {
            continue;
        }
        out = Some(match out {
            None => *r,
            Some(o) => {
                let x0 = o.x.min(r.x);
                let y0 = o.y.min(r.y);
                let x1 = (o.x + o.w).max(r.x + r.w);
                let y1 = (o.y + o.h).max(r.y + r.h);
                Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
            }
        });
    }
    out
}

fn viewport_size(vm: &Vm) -> (f32, f32, f32) {
    read(vm, |h| (h.width, h.height, h.scroll_y))
}

fn rect_value(vm: &mut Vm, r: Rect) -> Value {
    let make = read(vm, |h| h.hooks.get("rect").cloned());
    let args = vec![
        Value::Number(r.x as f64),
        Value::Number(r.y as f64),
        Value::Number(r.w as f64),
        Value::Number(r.h as f64),
    ];
    match make {
        Some(f) => vm.call(&f, Value::Undefined, args).unwrap_or(Value::Undefined),
        None => Value::Undefined,
    }
}

fn get_bounding_client_rect(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let (_, _, scroll) = viewport_size(vm);
    let r = rect_of(vm, n).map(|r| Rect { y: r.y - scroll, ..r }).unwrap_or(Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });
    Ok(rect_value(vm, r))
}

fn get_client_rects(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let (_, _, scroll) = viewport_size(vm);
    let items = match rect_of(vm, n) {
        Some(r) => vec![rect_value(vm, Rect { y: r.y - scroll, ..r })],
        None => Vec::new(),
    };
    Ok(vm.array_value(items))
}

fn is_root_el(vm: &Vm, n: NodeId) -> bool {
    read(vm, |h| h.page.doc.tag(n) == Some("html") && h.page.doc.node(n).parent == Some(Document::ROOT))
}

fn page_height(vm: &mut Vm) -> f32 {
    let h = host(vm);
    super::ensure_layout(&h);
    let hb = h.borrow();
    hb.layout.as_ref().map_or(hb.height, |l| l.height.max(hb.height))
}

fn offset_width(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Number(rect_of(vm, n).map_or(0.0, |r| r.w.round()) as f64))
}

fn offset_height(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Number(rect_of(vm, n).map_or(0.0, |r| r.h.round()) as f64))
}

fn offset_top(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Number(rect_of(vm, n).map_or(0.0, |r| r.y.round()) as f64))
}

fn offset_left(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Number(rect_of(vm, n).map_or(0.0, |r| r.x.round()) as f64))
}

fn offset_parent(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    if rect_of(vm, n).is_none() {
        return Ok(Value::Null);
    }
    let body = read(vm, |h| h.page.doc.find_first("body"));
    Ok(if body == Some(n) { Value::Null } else { wrap_opt(vm, body) })
}

/// Informações de rolagem do elemento (se ele rola por dentro).
fn scroller_of(vm: &mut Vm, n: NodeId) -> Option<crate::layout::Scroller> {
    let h = host(vm);
    super::ensure_layout(&h);
    let hb = h.borrow();
    let mut sc = hb.layout.as_ref()?.scrollers.iter().find(|s| s.node == n).copied()?;
    if let Some(&(x, y)) = hb.scroll_offsets.get(&n) {
        sc.x = x.clamp(0.0, sc.max_x);
        sc.y = y.clamp(0.0, sc.max_y);
    }
    Some(sc)
}

fn is_page_scroller(vm: &Vm, n: NodeId) -> bool {
    let body = read(vm, |h| h.page.doc.find_first("body"));
    is_root_el(vm, n) || Some(n) == body
}

fn client_width(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    if is_root_el(vm, n) {
        return Ok(Value::Number(viewport_size(vm).0 as f64));
    }
    if let Some(sc) = scroller_of(vm, n) {
        return Ok(Value::Number(sc.rect.w.round() as f64));
    }
    Ok(Value::Number(rect_of(vm, n).map_or(0.0, |r| r.w.round()) as f64))
}

fn client_height(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    if is_root_el(vm, n) {
        return Ok(Value::Number(viewport_size(vm).1 as f64));
    }
    if let Some(sc) = scroller_of(vm, n) {
        return Ok(Value::Number(sc.rect.h.round() as f64));
    }
    Ok(Value::Number(rect_of(vm, n).map_or(0.0, |r| r.h.round()) as f64))
}

fn scroll_width(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    if is_root_el(vm, n) {
        return Ok(Value::Number(viewport_size(vm).0 as f64));
    }
    if let Some(sc) = scroller_of(vm, n) {
        return Ok(Value::Number((sc.rect.w + sc.max_x).round() as f64));
    }
    Ok(Value::Number(rect_of(vm, n).map_or(0.0, |r| r.w.round()) as f64))
}

fn scroll_height(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    if is_page_scroller(vm, n) {
        return Ok(Value::Number(page_height(vm).round() as f64));
    }
    if let Some(sc) = scroller_of(vm, n) {
        return Ok(Value::Number((sc.rect.h + sc.max_y).round() as f64));
    }
    Ok(Value::Number(rect_of(vm, n).map_or(0.0, |r| r.h.round()) as f64))
}

fn zero(_vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Number(0.0))
}

fn scroll_top(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    if is_page_scroller(vm, n) {
        return Ok(Value::Number(viewport_size(vm).2 as f64));
    }
    Ok(Value::Number(scroller_of(vm, n).map_or(0.0, |s| s.y) as f64))
}

fn scroll_left(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    Ok(Value::Number(scroller_of(vm, n).map_or(0.0, |s| s.x) as f64))
}

/// Define a rolagem (x, y) de um elemento (ou da janela) e avisa com `scroll`.
fn set_scroll(vm: &mut Vm, n: NodeId, x: Option<f32>, y: Option<f32>) -> Result<(), Value> {
    if is_page_scroller(vm, n) {
        if let Some(y) = y {
            write(vm, |h| h.page_scroll_request = Some(y.max(0.0)));
        }
        return Ok(());
    }
    let Some(sc) = scroller_of(vm, n) else { return Ok(()) };
    let nx = x.unwrap_or(sc.x).clamp(0.0, sc.max_x);
    let ny = y.unwrap_or(sc.y).clamp(0.0, sc.max_y);
    if (nx - sc.x).abs() < 0.5 && (ny - sc.y).abs() < 0.5 {
        return Ok(());
    }
    write(vm, |h| {
        h.scroll_offsets.insert(n, (nx, ny));
        h.dirty = true;
    });
    if let Some(f) = read(vm, |h| h.hooks.get("fireLater").cloned()) {
        let el = wrap(vm, n);
        vm.call(&f, Value::Undefined, vec![el, s("scroll")])?;
    }
    Ok(())
}

fn set_scroll_top(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let y = vm.to_number(&a.arg(0))? as f32;
    set_scroll(vm, n, None, Some(if y.is_finite() { y } else { 0.0 }))?;
    Ok(Value::Undefined)
}

fn set_scroll_left(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let x = vm.to_number(&a.arg(0))? as f32;
    set_scroll(vm, n, Some(if x.is_finite() { x } else { 0.0 }), None)?;
    Ok(Value::Undefined)
}

/// `scrollTo(x, y)` / `scrollTo({left, top})` (e `scroll`).
fn scroll_to(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let (x, y) = match a.arg(0) {
        Value::Object(o) => {
            let l = vm.get_obj(&o, &ferro_js::value::PropKey::str("left"))?;
            let t = vm.get_obj(&o, &ferro_js::value::PropKey::str("top"))?;
            (
                if l.is_undefined() { None } else { Some(vm.to_number(&l)? as f32) },
                if t.is_undefined() { None } else { Some(vm.to_number(&t)? as f32) },
            )
        }
        v => (Some(vm.to_number(&v)? as f32), Some(vm.to_number(&a.arg(1))? as f32)),
    };
    set_scroll(vm, n, x.filter(|v| v.is_finite()), y.filter(|v| v.is_finite()))?;
    Ok(Value::Undefined)
}

fn scroll_by(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let (dx, dy) = match a.arg(0) {
        Value::Object(o) => {
            let l = vm.get_obj(&o, &ferro_js::value::PropKey::str("left"))?;
            let t = vm.get_obj(&o, &ferro_js::value::PropKey::str("top"))?;
            (
                if l.is_undefined() { 0.0 } else { vm.to_number(&l)? as f32 },
                if t.is_undefined() { 0.0 } else { vm.to_number(&t)? as f32 },
            )
        }
        v => (vm.to_number(&v)? as f32, vm.to_number(&a.arg(1))? as f32),
    };
    let (cx, cy) = scroller_of(vm, n).map_or((0.0, 0.0), |s| (s.x, s.y));
    set_scroll(vm, n, Some(cx + dx), Some(cy + dy))?;
    Ok(Value::Undefined)
}

/// `scrollIntoView()`: rola os ancestrais roláveis e a janela até o elemento.
fn scroll_into_view(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let Some(r) = rect_of(vm, n) else { return Ok(Value::Undefined) };
    let ancestors: Vec<NodeId> = read(vm, |h| {
        let mut out = Vec::new();
        let mut x = h.page.doc.node(n).parent;
        while let Some(p) = x {
            out.push(p);
            x = h.page.doc.node(p).parent;
        }
        out
    });
    for p in ancestors {
        if let Some(sc) = scroller_of(vm, p)
            && (r.y < sc.rect.y || r.y + r.h > sc.rect.y + sc.rect.h)
        {
            set_scroll(vm, p, None, Some(sc.y + (r.y - sc.rect.y)))?;
        }
    }
    let r = rect_of(vm, n).unwrap_or(r);
    let (_, vh, sy) = viewport_size(vm);
    if r.y < sy || r.y + r.h > sy + vh {
        write(vm, |h| h.page_scroll_request = Some((r.y - 8.0).max(0.0)));
    }
    Ok(Value::Undefined)
}

fn noop(_vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Undefined)
}

// --------------------------------------------------------- HTMLElement

/// innerText aproximado: o texto sem `<script>`/`<style>` e com quebras nos blocos.
fn inner_text(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let text = read(vm, |h| {
        let doc = &h.page.doc;
        let mut out = String::new();
        fn walk(doc: &Document, n: NodeId, out: &mut String) {
            match &doc.node(n).data {
                NodeData::Text(t) => {
                    let collapsed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !collapsed.is_empty() {
                        if t.starts_with(char::is_whitespace) && !out.is_empty() && !out.ends_with(['\n', ' ']) {
                            out.push(' ');
                        }
                        out.push_str(&collapsed);
                        if t.ends_with(char::is_whitespace) {
                            out.push(' ');
                        }
                    }
                }
                NodeData::Element(e) => {
                    let tag = e.tag.as_str();
                    if matches!(tag, "script" | "style" | "template" | "noscript" | "head") {
                        return;
                    }
                    if tag == "br" {
                        while out.ends_with(' ') {
                            out.pop();
                        }
                        out.push('\n');
                        return;
                    }
                    let block = matches!(
                        tag,
                        "div" | "p" | "li" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "tr" | "table"
                            | "section" | "article" | "header" | "footer" | "nav" | "form" | "pre" | "blockquote"
                    );
                    if block && !out.is_empty() && !out.ends_with('\n') {
                        while out.ends_with(' ') {
                            out.pop();
                        }
                        out.push('\n');
                    }
                    for &c in &doc.node(n).children {
                        walk(doc, c, out);
                    }
                    if block && !out.ends_with('\n') {
                        while out.ends_with(' ') {
                            out.pop();
                        }
                        out.push('\n');
                    }
                }
                _ => {}
            }
        }
        walk(doc, n, &mut out);
        out.trim().to_string()
    });
    Ok(s(&text))
}

fn set_inner_text(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let text = if a.arg(0).is_nullish() { String::new() } else { arg_string(vm, a, 0)? };
    removing_children(vm, n);
    let kids = write(vm, |h| {
        h.page.doc.remove_children(n);
        let mut first = true;
        for line in text.split('\n') {
            if !first {
                let br = h.page.doc.create_element("br");
                h.page.doc.append(n, br);
            }
            first = false;
            if !line.is_empty() {
                let t = h.page.doc.create(NodeData::Text(line.to_string()));
                h.page.doc.append(n, t);
            }
        }
        h.page.doc.node(n).children.clone()
    });
    inserted(vm, &kids, n);
    Ok(Value::Undefined)
}

fn focus(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let prev = write(vm, |h| {
        h.repaint = true;
        let prev = h.focused.replace(n);
        crate::css::set_dynamic_state(super::dynamic_state(h));
        prev
    });
    if prev == Some(n) {
        return Ok(Value::Undefined);
    }
    let fire = read(vm, |h| h.hooks.get("fire").cloned());
    if let Some(f) = fire {
        if let Some(p) = prev {
            let el = wrap(vm, p);
            vm.call(&f, Value::Undefined, vec![el, s("blur")])?;
        }
        let el = wrap(vm, n);
        vm.call(&f, Value::Undefined, vec![el.clone(), s("focus")])?;
        vm.call(&f, Value::Undefined, vec![el, s("focusin"), Value::Bool(true)])?;
    }
    Ok(Value::Undefined)
}

fn blur(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let was = write(vm, |h| {
        if h.focused == Some(n) {
            h.repaint = true;
            h.focused = None;
            crate::css::set_dynamic_state(super::dynamic_state(h));
            true
        } else {
            false
        }
    });
    if was && let Some(f) = read(vm, |h| h.hooks.get("fire").cloned()) {
        let el = wrap(vm, n);
        vm.call(&f, Value::Undefined, vec![el.clone(), s("blur")])?;
        vm.call(&f, Value::Undefined, vec![el, s("focusout"), Value::Bool(true)])?;
    }
    Ok(Value::Undefined)
}

/// `el.click()`: dispara o clique e, se não for cancelado, a ação padrão.
fn click(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let Some(f) = read(vm, |h| h.hooks.get("click").cloned()) else { return Ok(Value::Undefined) };
    let el = wrap(vm, n);
    let ok = vm.call(&f, Value::Undefined, vec![el, Value::Number(0.0), Value::Number(0.0)])?;
    if ferro_js::vm::truthy(&ok) {
        let target = write(vm, |h| {
            let doc = &h.page.doc;
            let el = doc.element(n)?;
            match el.tag.as_str() {
                "a" | "area" => el.attr("href").and_then(|href| h.page.base.join(href.trim()).ok()),
                _ => None,
            }
        });
        if let Some(u) = target {
            write(vm, |h| h.navigate = Some(u));
        }
        let toggle = read(vm, |h| {
            let el = h.page.doc.element(n)?;
            (el.tag == "input").then(|| el.attr("type").unwrap_or("").to_ascii_lowercase())
        });
        match toggle.as_deref() {
            Some("checkbox") => {
                let on = attr_of(vm, n, "checked").is_some();
                write(vm, |h| {
                    if on {
                        h.page.doc.remove_attr(n, "checked");
                    } else {
                        h.page.doc.set_attr(n, "checked", "");
                    }
                    h.dirty = true;
                });
            }
            Some("radio") => write(vm, |h| {
                h.page.doc.set_attr(n, "checked", "");
                h.dirty = true;
            }),
            _ => {}
        }
    }
    Ok(Value::Undefined)
}

/// `el.attachShadow({mode})`: cria a raiz de sombra.
fn attach_shadow(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let closed = match a.arg(0) {
        Value::Object(o) => {
            let m = vm.get_obj(&o, &ferro_js::value::PropKey::str("mode"))?;
            vm.to_rust_string(&m)? == "closed"
        }
        _ => false,
    };
    if read(vm, |h| h.page.doc.shadow_roots.contains_key(&n)) {
        return Err(dom_error(
            vm,
            "NotSupportedError",
            "Failed to execute 'attachShadow' on 'Element': Shadow root cannot be created on a host which already hosts a shadow tree.",
        ));
    }
    let root = write(vm, |h| {
        h.dirty = true;
        if closed {
            h.closed_shadows.insert(n);
        }
        h.page.doc.attach_shadow(n)
    });
    Ok(wrap(vm, root))
}

fn shadow_root(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let r = read(vm, |h| {
        if h.closed_shadows.contains(&n) { None } else { h.page.doc.shadow_roots.get(&n).copied() }
    });
    Ok(wrap_opt(vm, r))
}

fn shadow_host(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let h = read(vm, |h| h.page.doc.shadow_hosts.get(&n).copied());
    Ok(wrap_opt(vm, h))
}

fn shadow_mode(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let closed = read(vm, |h| h.page.doc.shadow_hosts.get(&n).is_some_and(|host| h.closed_shadows.contains(host)));
    Ok(s(if closed { "closed" } else { "open" }))
}

/// `slot.assignedNodes()`.
fn assigned_nodes(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let list = read(vm, |h| {
        let doc = &h.page.doc;
        doc.slot_host(n).map(|host| doc.assigned_nodes(host, n)).unwrap_or_default()
    });
    let items: Vec<Value> = list.into_iter().map(|x| wrap(vm, x)).collect();
    Ok(vm.array_value(items))
}

fn assigned_elements(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let list = read(vm, |h| {
        let doc = &h.page.doc;
        doc.slot_host(n)
            .map(|host| doc.assigned_nodes(host, n).into_iter().filter(|&x| is_element(doc, x)).collect())
            .unwrap_or_default()
    });
    Ok(list_plain(vm, list))
}

fn list_plain(vm: &mut Vm, ids: Vec<NodeId>) -> Value {
    let items: Vec<Value> = ids.into_iter().map(|x| wrap(vm, x)).collect();
    vm.array_value(items)
}

/// `template.content`: um fragmento com os filhos do template.
fn template_content(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = this(vm, a)?;
    let frag = write(vm, |h| {
        if let Some(&f) = h.templates.get(&n) {
            return f;
        }
        let f = h.page.doc.create_fragment();
        let kids = std::mem::take(&mut h.page.doc.nodes[n].children);
        for k in kids {
            h.page.doc.nodes[k].parent = None;
            h.page.doc.append(f, k);
        }
        h.templates.insert(n, f);
        f
    });
    Ok(wrap(vm, frag))
}

// --------------------------------------------------------------- Document

fn find_tag(vm: &Vm, tag: &str) -> Option<NodeId> {
    find_tag_in(vm, Document::ROOT, tag)
}

/// `<html>`, `<head>` ou `<body>` de um documento (o principal ou um criado
/// por createHTMLDocument/DOMParser).
fn find_tag_in(vm: &Vm, root: NodeId, tag: &str) -> Option<NodeId> {
    read(vm, |h| {
        let doc = &h.page.doc;
        let html = doc.node(root).children.iter().copied().find(|&c| doc.tag(c) == Some("html"))?;
        if tag == "html" {
            return Some(html);
        }
        doc.node(html).children.iter().copied().find(|&c| doc.tag(c) == Some(tag))
    })
}

fn document_element(vm: &mut Vm, a: &mut CallArgs) -> R {
    let d = this(vm, a)?;
    let n = find_tag_in(vm, d, "html");
    Ok(wrap_opt(vm, n))
}

fn head(vm: &mut Vm, a: &mut CallArgs) -> R {
    let d = this(vm, a)?;
    let n = find_tag_in(vm, d, "head");
    Ok(wrap_opt(vm, n))
}

fn body(vm: &mut Vm, a: &mut CallArgs) -> R {
    let d = this(vm, a)?;
    let n = find_tag_in(vm, d, "body");
    Ok(wrap_opt(vm, n))
}

/// Documento novo, fora da árvore principal (createHTMLDocument).
fn new_document(vm: &mut Vm, html: &str) -> NodeId {
    write(vm, |h| {
        let tmp = crate::html::parse(html);
        let d = h.page.doc.create(NodeData::Document);
        for &c in &tmp.node(Document::ROOT).children.clone() {
            let k = crate::html::copy_into(&tmp, c, &mut h.page.doc);
            h.page.doc.append(d, k);
        }
        // scripts de documentos inertes nunca rodam
        for x in h.page.doc.descendants(d) {
            if h.page.doc.tag(x) == Some("script") {
                h.started.insert(x);
            }
        }
        d
    })
}

/// `__native.createDocument(título)`.
pub(crate) fn create_document(vm: &mut Vm, a: &mut CallArgs) -> R {
    let title = if a.arg(0).is_nullish() {
        String::new()
    } else {
        let t = arg_string(vm, a, 0)?;
        format!("<title>{}</title>", t.replace('&', "&amp;").replace('<', "&lt;"))
    };
    let d = new_document(vm, &format!("<!DOCTYPE html><html><head>{title}</head><body></body></html>"));
    Ok(wrap(vm, d))
}

/// `__native.parseDocument(html)` (DOMParser).
pub(crate) fn parse_document(vm: &mut Vm, a: &mut CallArgs) -> R {
    let html = arg_string(vm, a, 0)?;
    let d = new_document(vm, &html);
    Ok(wrap(vm, d))
}

fn doctype(vm: &mut Vm, _a: &mut CallArgs) -> R {
    let n = read(vm, |h| {
        let doc = &h.page.doc;
        doc.node(Document::ROOT).children.iter().copied().find(|&c| matches!(doc.node(c).data, NodeData::Doctype(_)))
    });
    Ok(wrap_opt(vm, n))
}

fn title(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(s(&read(vm, |h| {
        let doc = &h.page.doc;
        doc.find_first("title")
            .map(|t| doc.text_content(t).split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    })))
}

fn set_title(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = arg_string(vm, a, 0)?;
    write(vm, |h| {
        let t = match h.page.doc.find_first("title") {
            Some(t) => t,
            None => {
                let t = h.page.doc.create_element("title");
                let head = h.page.doc.find_first("head").unwrap_or(Document::ROOT);
                h.page.doc.append(head, t);
                t
            }
        };
        h.page.doc.set_text_content(t, &text);
        h.title_changed = true;
    });
    Ok(Value::Undefined)
}

fn document_url(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(s(read(vm, |h| h.page.url.to_string()).as_str()))
}

fn ready_state(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(s(read(vm, |h| h.ready_state)))
}

fn current_script(vm: &mut Vm, _a: &mut CallArgs) -> R {
    let n = read(vm, |h| h.current_script);
    Ok(wrap_opt(vm, n))
}

fn active_element(vm: &mut Vm, _a: &mut CallArgs) -> R {
    let n = read(vm, |h| h.focused.filter(|&f| h.page.doc.is_connected(f)));
    let n = n.or_else(|| find_tag(vm, "body"));
    Ok(wrap_opt(vm, n))
}

fn default_view(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Object(vm.realm.global.clone()))
}

fn create_element(vm: &mut Vm, a: &mut CallArgs) -> R {
    let tag = arg_string(vm, a, 0)?.to_ascii_lowercase();
    if tag.is_empty() || tag.contains(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"' || c == '/') {
        return Err(dom_error(
            vm,
            "InvalidCharacterError",
            &format!("The tag name provided ('{tag}') is not a valid name."),
        ));
    }
    let n = write(vm, |h| h.page.doc.create_element(&tag));
    let el = wrap(vm, n);
    upgrade_custom(vm, n, &el)?;
    Ok(el)
}

fn create_element_ns(vm: &mut Vm, a: &mut CallArgs) -> R {
    let tag = arg_string(vm, a, 1)?;
    let tag = tag.rsplit(':').next().unwrap_or("").to_string();
    let n = write(vm, |h| h.page.doc.create_element(&tag.to_ascii_lowercase()));
    Ok(wrap(vm, n))
}

fn create_text_node(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = arg_string(vm, a, 0)?;
    let n = write(vm, |h| h.page.doc.create(NodeData::Text(text)));
    Ok(wrap(vm, n))
}

fn create_comment(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = arg_string(vm, a, 0)?;
    let n = write(vm, |h| h.page.doc.create(NodeData::Comment(text)));
    Ok(wrap(vm, n))
}

fn create_document_fragment(vm: &mut Vm, _a: &mut CallArgs) -> R {
    let n = write(vm, |h| h.page.doc.create_fragment());
    Ok(wrap(vm, n))
}

fn import_node(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = arg_node(vm, a, 0)?;
    let deep = ferro_js::vm::truthy(&a.arg(1));
    let c = write(vm, |h| h.page.doc.clone_node(n, deep));
    try_upgrade(vm, &[c]);
    Ok(wrap(vm, c))
}

fn adopt_node(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = arg_node(vm, a, 0)?;
    removing(vm, n);
    write(vm, |h| h.page.doc.detach(n));
    Ok(a.arg(0))
}

/// `document.write`: durante o carregamento, o HTML entra logo depois do
/// script que está rodando.
fn document_write(vm: &mut Vm, a: &mut CallArgs) -> R {
    let mut html = String::new();
    for v in &a.args {
        html.push_str(&vm.to_rust_string(v)?);
    }
    let (cur, loading) = read(vm, |h| (h.current_script, h.ready_state == "loading"));
    let (parent, before) = match cur.filter(|_| loading) {
        Some(sc) => {
            let p = read(vm, |h| h.page.doc.node(sc).parent);
            let Some(p) = p else { return Ok(Value::Undefined) };
            (p, read(vm, |h| sibling(&h.page.doc, sc, 1, false)))
        }
        None => match find_tag(vm, "body") {
            Some(b) => (b, None),
            None => return Ok(Value::Undefined),
        },
    };
    let nodes = write(vm, |h| {
        let ctx = fragment_context(&h.page.doc, parent);
        let nodes = crate::html::parse_fragment(&html, &ctx, &mut h.page.doc);
        for &c in &nodes {
            h.page.doc.insert_before(parent, c, before);
        }
        nodes
    });
    // scripts escritos durante o carregamento rodam no laço do parser
    if loading {
        write(vm, |h| h.dirty = true);
    } else {
        inserted(vm, &nodes, parent);
    }
    Ok(Value::Undefined)
}

fn document_writeln(vm: &mut Vm, a: &mut CallArgs) -> R {
    a.args.push(s("\n"));
    document_write(vm, a)
}

fn element_from_point(vm: &mut Vm, a: &mut CallArgs) -> R {
    let x = vm.to_number(&a.arg(0))? as f32;
    let y = vm.to_number(&a.arg(1))? as f32;
    let h = host(vm);
    super::ensure_layout(&h);
    let scroll = h.borrow().scroll_y;
    let found = {
        let hb = h.borrow();
        let doc = &hb.page.doc;
        hb.layout.as_ref().and_then(|info| {
            let depth = |mut n: NodeId| {
                let mut d = 0;
                while let Some(p) = doc.node(n).parent {
                    d += 1;
                    n = p;
                }
                d
            };
            info.boxes
                .iter()
                .filter(|(n, r)| {
                    let yy = y + scroll;
                    x >= r.x && x < r.x + r.w && yy >= r.y && yy < r.y + r.h && doc.is_connected(*n)
                })
                .max_by_key(|(n, _)| depth(*n))
                .map(|(n, _)| *n)
        })
    };
    let found = found.or_else(|| find_tag(vm, "html"));
    Ok(wrap_opt(vm, found))
}

fn has_focus(_vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Bool(true))
}

fn collection_of(vm: &mut Vm, test: impl Fn(&Document, NodeId) -> bool) -> Value {
    let found = read(vm, |h| {
        let doc = &h.page.doc;
        doc.descendants(Document::ROOT).into_iter().filter(|&d| test(doc, d)).collect()
    });
    list(vm, found, "HTMLCollection")
}

fn forms(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(collection_of(vm, |d, n| d.tag(n) == Some("form")))
}

fn images(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(collection_of(vm, |d, n| d.tag(n) == Some("img")))
}

fn scripts(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(collection_of(vm, |d, n| d.tag(n) == Some("script")))
}

fn links(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(collection_of(vm, |d, n| {
        matches!(d.tag(n), Some("a" | "area")) && d.element(n).is_some_and(|e| e.attr("href").is_some())
    }))
}

// ------------------------------------------------------------ construtores

fn illegal(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Err(vm.type_error("Illegal constructor"))
}

fn event_target_ctor(vm: &mut Vm, a: &mut CallArgs) -> R {
    let def = read(vm, |h| h.protos.get("EventTarget").cloned()).expect("EventTarget");
    let proto = vm.proto_from_ctor(a.new_target.as_ref(), &def)?;
    Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::Ordinary))))
}

fn text_ctor(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = if a.arg(0).is_undefined() { String::new() } else { arg_string(vm, a, 0)? };
    let n = write(vm, |h| h.page.doc.create(NodeData::Text(text)));
    Ok(wrap(vm, n))
}

fn comment_ctor(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = if a.arg(0).is_undefined() { String::new() } else { arg_string(vm, a, 0)? };
    let n = write(vm, |h| h.page.doc.create(NodeData::Comment(text)));
    Ok(wrap(vm, n))
}

fn fragment_ctor(vm: &mut Vm, _a: &mut CallArgs) -> R {
    let n = write(vm, |h| h.page.doc.create_fragment());
    Ok(wrap(vm, n))
}

fn image_ctor(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = write(vm, |h| h.page.doc.create_element("img"));
    for (i, name) in ["width", "height"].iter().enumerate() {
        if !a.arg(i).is_undefined() {
            let v = vm.to_rust_string(&a.arg(i))?;
            write(vm, |h| h.page.doc.set_attr(n, name, &v));
        }
    }
    Ok(wrap(vm, n))
}

fn option_ctor(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = write(vm, |h| h.page.doc.create_element("option"));
    if !a.arg(0).is_undefined() {
        let t = arg_string(vm, a, 0)?;
        write(vm, |h| h.page.doc.set_text_content(n, &t));
    }
    if !a.arg(1).is_undefined() {
        let v = arg_string(vm, a, 1)?;
        write(vm, |h| h.page.doc.set_attr(n, "value", &v));
    }
    if ferro_js::vm::truthy(&a.arg(3)) {
        write(vm, |h| h.page.doc.set_attr(n, "selected", ""));
    }
    Ok(wrap(vm, n))
}

/// `HTMLElement` só pode ser construído por elementos personalizados
/// (`class X extends HTMLElement`, depois de `customElements.define`).
fn html_element_ctor(vm: &mut Vm, a: &mut CallArgs) -> R {
    let Some(nt) = a.new_target.clone() else { return Err(vm.type_error("Illegal constructor")) };
    // elemento esperando a atualização (upgrade)?
    if let Some(n) = read(vm, |h| h.hooks.get("__upgrading").and_then(node_of)) {
        write(vm, |h| h.hooks.remove("__upgrading"));
        let el = wrap(vm, n);
        let proto = vm.get_obj(&nt, &ferro_js::value::PropKey::str("prototype"))?;
        if let (Value::Object(o), Value::Object(p)) = (&el, proto) {
            o.borrow_mut().proto = Some(p);
        }
        return Ok(el);
    }
    let tag = read(vm, |h| h.custom.iter().find(|(_, c)| c.ptr_eq(&nt)).map(|(t, _)| t.clone()));
    let Some(tag) = tag else { return Err(vm.type_error("Illegal constructor")) };
    let n = write(vm, |h| h.page.doc.create_element(&tag));
    let el = wrap(vm, n);
    Ok(el)
}

/// Elemento novo com tag de um elemento personalizado: roda o construtor.
pub(crate) fn upgrade_custom(vm: &mut Vm, n: NodeId, el: &Value) -> Result<(), Value> {
    let ctor = read(vm, |h| h.page.doc.tag(n).and_then(|t| h.custom.get(t).cloned()));
    let Some(ctor) = ctor else { return Ok(()) };
    if !write(vm, |h| h.upgraded.insert(n)) {
        return Ok(());
    }
    write(vm, |h| h.hooks.insert("__upgrading", el.clone()));
    let r = vm.construct(&ctor, Vec::new(), &ctor);
    write(vm, |h| h.hooks.remove("__upgrading"));
    r.map(|_| ())
}

/// `window.algumId`: elemento com esse id (acesso nomeado da janela).
fn named_element(vm: &mut Vm, name: &ferro_js::JsStr) -> Option<Value> {
    let name = name.to_rust();
    let found = read(vm, |h| {
        let doc = &h.page.doc;
        doc.descendants(Document::ROOT)
            .into_iter()
            .find(|&d| doc.element(d).is_some_and(|e| e.id() == Some(name.as_str())))
    })?;
    Some(wrap(vm, found))
}

// ------------------------------------------------------------- instalação

struct Protos<'a> {
    vm: &'a mut Vm,
}

impl Protos<'_> {
    fn make(&mut self, name: &'static str, parent: &ObjRef, ctor: NativeFn) -> ObjRef {
        let p = ObjRef::new(Object::new(Some(parent.clone()), Kind::Ordinary));
        self.vm.realm.tag(&p, name);
        let c = self.vm.realm.ctor(name, 0, ctor, &p);
        // a cadeia dos construtores também segue a herança (HTMLDivElement.__proto__ === HTMLElement)
        let from_object = parent.ptr_eq(&self.vm.realm.object_proto);
        if !from_object
            && let Some(Value::Object(pc)) = parent.borrow().own(&ferro_js::value::PropKey::str("constructor")).and_then(|pr| match pr.slot {
                ferro_js::value::Slot::Data { value, .. } => Some(value),
                _ => None,
            })
            && pc.is_callable()
        {
            c.borrow_mut().proto = Some(pc);
        }
        host(self.vm).borrow_mut().protos.insert(name, p.clone());
        p
    }

    fn get(&mut self, p: &ObjRef, name: &str, f: NativeFn) {
        self.vm.realm.accessor(p, name, Some(f), None);
    }

    fn get_set(&mut self, p: &ObjRef, name: &str, g: NativeFn, s: NativeFn) {
        self.vm.realm.accessor(p, name, Some(g), Some(s));
    }

    fn method(&mut self, p: &ObjRef, name: &str, len: u32, f: NativeFn) {
        self.vm.realm.method(p, name, len, f);
    }
}

