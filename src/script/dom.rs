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

