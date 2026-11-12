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

fn uint8_array(vm: &mut Vm, bytes: &[u8]) -> R {
    let o = ferro_js::builtins::typed::new_typed(vm, TaKind::U8, bytes.len(), None)?;
    if let Kind::TypedArray(t) = &o.borrow().kind {
        t.data.borrow_mut().copy_from_slice(bytes);
    }
    Ok(Value::Object(o))
}

fn utf8_encode(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = if a.arg(0).is_undefined() { String::new() } else { arg_string(vm, a, 0)? };
    uint8_array(vm, text.as_bytes())
}

fn utf8_decode(vm: &mut Vm, a: &mut CallArgs) -> R {
    let bytes = bytes_of(&a.arg(0)).unwrap_or_default();
    let label = if a.arg(1).is_undefined() { "utf-8".to_string() } else { arg_string(vm, a, 1)? };
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    Ok(s(&net::decode_text(bytes, Some(&format!("text/plain; charset={label}")), false)))
}

// ----------------------------------------------------------------- base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn btoa(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = arg_string(vm, a, 0)?;
    let mut bytes = Vec::with_capacity(text.len());
    for c in text.chars() {
        if c as u32 > 255 {
            return Err(super::dom::dom_error(
                vm,
                "InvalidCharacterError",
                "Failed to execute 'btoa' on 'Window': The string to be encoded contains characters outside of the Latin1 range.",
            ));
        }
        bytes.push(c as u8);
    }
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    Ok(s(&out))
}

fn atob(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = arg_string(vm, a, 0)?;
    let clean: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let clean = if clean.len() % 4 == 0 {
        let mut c = clean;
        while c.last() == Some(&b'=') {
            c.pop();
        }
        c
    } else {
        clean
    };
    let bad = || "Failed to execute 'atob' on 'Window': The string to be decoded is not correctly encoded.";
    if clean.len() % 4 == 1 {
        return Err(super::dom::dom_error(vm, "InvalidCharacterError", bad()));
    }
    let mut out = String::with_capacity(clean.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in &clean {
        let v = match B64.iter().position(|&x| x == c) {
            Some(v) => v as u32,
            None => return Err(super::dom::dom_error(vm, "InvalidCharacterError", bad())),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(char::from((acc >> bits) as u8));
            acc &= (1 << bits) - 1;
        }
    }
    Ok(s(&out))
}

// -------------------------------------------------------------------- rede

/// Objeto com o resultado de uma requisição (o prelúdio monta o Response).
pub(crate) fn response_value(vm: &mut Vm, res: Result<net::Response, String>) -> Value {
    let o = vm.new_object();
    match res {
        Ok(r) => {
            set(vm, &o, "status", Value::Number(r.status as f64));
            set(vm, &o, "url", s(r.url.as_str()));
            let headers: Vec<Value> = r
                .headers
                .iter()
                .filter(|(k, _)| k != "set-cookie")
                .map(|(k, v)| vm.array_value(vec![s(k), s(v)]))
                .collect();
            let h = vm.array_value(headers);
            set(vm, &o, "headers", h);
            let bytes = uint8_array(vm, &r.body).unwrap_or(Value::Null);
            set(vm, &o, "body", bytes);
            set(vm, &o, "contentType", r.content_type.as_deref().map_or(Value::Null, s));
        }
        Err(e) => {
            set(vm, &o, "error", s(&e));
        }
    }
    Value::Object(o)
}

/// Monta a requisição a partir de (método, url, [[nome, valor]...], corpo).
fn request_from_args(vm: &mut Vm, a: &CallArgs) -> Result<net::Request, Value> {
    let method = arg_string(vm, a, 0)?;
    let url_s = arg_string(vm, a, 1)?;
    let base = read(vm, |h| h.page.base.clone());
    let url = base
        .join(&url_s)
        .map_err(|_| vm.type_error(format!("Failed to fetch: invalid URL '{url_s}'")))?;
    let mut headers = Vec::new();
    if let Value::Object(list) = a.arg(2) {
        let items = vm.iterate_to_vec(&Value::Object(list))?;
        for it in items {
            let pair = vm.iterate_to_vec(&it)?;
            if pair.len() >= 2 {
                let k = vm.to_rust_string(&pair[0])?;
                let v = vm.to_rust_string(&pair[1])?;
                headers.push((k, v));
            }
        }
    }
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept")) {
        headers.push(("Accept".into(), "*/*".into()));
    }
    let body = match a.arg(3) {
        Value::Undefined | Value::Null => None,
        v => match bytes_of(&v) {
            Some(b) => Some(b),
            None => Some(vm.to_rust_string(&v)?.into_bytes()),
        },
    };
    // Referer e Origin, como um navegador manda
    let page = read(vm, |h| h.page.url.clone());
    if matches!(page.scheme(), "http" | "https") {
        headers.push(("Referer".into(), page.to_string()));
        if method != "GET" && method != "HEAD" {
            headers.push(("Origin".into(), page.origin().ascii_serialization()));
        }
    }
    let cookies = a.arg(4).is_undefined() || ferro_js::vm::truthy(&a.arg(4));
    Ok(net::Request {
        method,
        url,
        headers,
        body,
        cookies,
    })
}

/// `__native.fetch(método, url, cabeçalhos, corpo, comCookies, callback)`.
fn fetch(vm: &mut Vm, a: &mut CallArgs) -> R {
    let req = request_from_args(vm, a)?;
    let cb = a.arg(5);
    write(vm, |h| {
        h.fetch_seq += 1;
        let id = h.fetch_seq;
        h.fetch_waiters.insert(id, cb);
        let net = h.net.clone();
        h.in_flight += 1;
        h.pool.submit(Box::new(move || Done::Fetch { id, res: net.request(&req) }));
    });
    Ok(Value::Undefined)
}

/// XMLHttpRequest síncrono (ainda usado por sites antigos).
fn fetch_sync(vm: &mut Vm, a: &mut CallArgs) -> R {
    let req = request_from_args(vm, a)?;
    let net = read(vm, |h| h.net.clone());
    let res = net.request(&req);
    Ok(response_value(vm, res))
}

// ------------------------------------------------------------------- URL

fn url_object(vm: &mut Vm, u: &Url) -> Value {
    let o = vm.new_object();
    let port = u.port().map(|p| p.to_string()).unwrap_or_default();
    let host = match u.host_str() {
        Some(h) if !port.is_empty() => format!("{h}:{port}"),
        Some(h) => h.to_string(),
        None => String::new(),
    };
    let search = u.query().filter(|q| !q.is_empty()).map(|q| format!("?{q}")).unwrap_or_default();
    let hash = u.fragment().filter(|f| !f.is_empty()).map(|f| format!("#{f}")).unwrap_or_default();
    let origin = u.origin().ascii_serialization();
    for (k, v) in [
        ("href", u.as_str().to_string()),
        ("protocol", format!("{}:", u.scheme())),
        ("username", u.username().to_string()),
        ("password", u.password().unwrap_or("").to_string()),
        ("host", host),
        ("hostname", u.host_str().unwrap_or("").to_string()),
        ("port", port),
        ("pathname", u.path().to_string()),
        ("search", search),
        ("hash", hash),
        ("origin", origin),
    ] {
        set(vm, &o, k, s(&v));
    }
    Value::Object(o)
}

/// `__native.urlParse(entrada, base?)` → partes da URL ou null.
fn url_parse(vm: &mut Vm, a: &mut CallArgs) -> R {
    let input = arg_string(vm, a, 0)?;
    let parsed = if a.arg(1).is_undefined() || a.arg(1).is_nullish() {
        Url::parse(input.trim())
    } else {
        let base = arg_string(vm, a, 1)?;
        match Url::parse(base.trim()) {
            Ok(b) => b.join(input.trim()),
            Err(e) => Err(e),
        }
    };
    match parsed {
        Ok(u) => Ok(url_object(vm, &u)),
        Err(_) => Ok(Value::Null),
    }
}

/// `__native.urlSet(href, parte, valor)` → nova href.
fn url_set(vm: &mut Vm, a: &mut CallArgs) -> R {
    let href = arg_string(vm, a, 0)?;
    let part = arg_string(vm, a, 1)?;
    let value = arg_string(vm, a, 2)?;
    let Ok(mut u) = Url::parse(&href) else { return Ok(s(&href)) };
    match part.as_str() {
        "protocol" => {
            let _ = u.set_scheme(value.trim_end_matches(':'));
        }
        "username" => {
            let _ = u.set_username(&value);
        }
        "password" => {
            let _ = u.set_password(Some(&value));
        }
        "host" => {
            let (h, p) = match value.rsplit_once(':') {
                Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h.to_string(), p.parse().ok()),
                _ => (value.clone(), None),
            };
            if u.set_host(Some(&h)).is_ok() {
                let _ = u.set_port(p);
            }
        }
        "hostname" => {
            let _ = u.set_host(Some(&value));
        }
        "port" => {
            let _ = u.set_port(value.parse().ok());
        }
        "pathname" => u.set_path(&value),
        "search" => {
            let q = value.trim_start_matches('?');
            u.set_query(if q.is_empty() { None } else { Some(q) });
        }
        "hash" => {
            let f = value.trim_start_matches('#');
            u.set_fragment(if f.is_empty() { None } else { Some(f) });
        }
        _ => {}
    }
    Ok(s(u.as_str()))
}

/// application/x-www-form-urlencoded: decodifica um componente.
fn form_decode(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = arg_string(vm, a, 0)?.replace('+', " ");
    Ok(s(&String::from_utf8_lossy(&net::percent_decode(&text))))
}

fn form_encode(vm: &mut Vm, a: &mut CallArgs) -> R {
    let text = arg_string(vm, a, 0)?;
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    Ok(s(&out))
}

// --------------------------------------------------------------- location

fn location_href(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(s(read(vm, |h| h.page.url.to_string()).as_str()))
}

/// `__native.navigate(url)`: a aba carrega outra página (ou só muda o #).
fn navigate(vm: &mut Vm, a: &mut CallArgs) -> R {
    let target = arg_string(vm, a, 0)?;
    let (cur, base) = read(vm, |h| (h.page.url.clone(), h.page.base.clone()));
    let Ok(url) = base.join(target.trim()) else {
        return Err(super::dom::dom_error(vm, "SyntaxError", &format!("'{target}' is not a valid URL.")));
    };
    let mut a_no = url.clone();
    a_no.set_fragment(None);
    let mut b_no = cur.clone();
    b_no.set_fragment(None);
    if a_no == b_no && url.fragment().is_some() {
        // só o fragmento: sem recarregar
        write(vm, |h| h.page.url = url);
        return Ok(Value::Bool(false));
    }
    if url.scheme() == "javascript" {
        return Ok(Value::Bool(false));
    }
    write(vm, |h| h.navigate = Some(url));
    Ok(Value::Bool(true))
}

/// history.pushState/replaceState: troca a URL sem carregar (mesma origem).
fn set_url(vm: &mut Vm, a: &mut CallArgs) -> R {
    let target = arg_string(vm, a, 0)?;
    let (cur, base) = read(vm, |h| (h.page.url.clone(), h.page.base.clone()));
    let Ok(url) = base.join(target.trim()) else {
        return Err(super::dom::dom_error(vm, "SecurityError", "Invalid URL"));
    };
    if url.origin() != cur.origin() {
        return Err(super::dom::dom_error(
            vm,
            "SecurityError",
            &format!("A history state object with URL '{url}' cannot be created in a document with origin '{}'.", cur.origin().ascii_serialization()),
        ));
    }
    write(vm, |h| h.page.url = url);
    Ok(Value::Undefined)
}

/// `__native.navigatePost(url, content-type, corpo)`: envio de formulário por POST.
fn navigate_post(vm: &mut Vm, a: &mut CallArgs) -> R {
    let target = arg_string(vm, a, 0)?;
    let ct = arg_string(vm, a, 1)?;
    let body = match bytes_of(&a.arg(2)) {
        Some(b) => b,
        None => arg_string(vm, a, 2)?.into_bytes(),
    };
    let base = read(vm, |h| h.page.base.clone());
    let Ok(url) = base.join(target.trim()) else {
        return Err(super::dom::dom_error(vm, "SyntaxError", &format!("'{target}' is not a valid URL.")));
    };
    write(vm, |h| {
        h.navigate = Some(url);
        h.post = Some((ct, body));
    });
    Ok(Value::Undefined)
}

fn reload(vm: &mut Vm, _a: &mut CallArgs) -> R {
    write(vm, |h| h.navigate = Some(h.page.url.clone()));
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------- cookies

fn cookie(vm: &mut Vm, _a: &mut CallArgs) -> R {
    let url = read(vm, |h| h.page.url.clone());
    Ok(s(&crate::cookies::for_script(&url)))
}

fn set_cookie(vm: &mut Vm, a: &mut CallArgs) -> R {
    let line = arg_string(vm, a, 0)?;
    let url = read(vm, |h| h.page.url.clone());
    crate::cookies::store_from_script(&url, &line);
    Ok(Value::Undefined)
}

// ----------------------------------------------------------- armazenamento

/// localStorage: um mapa por origem, para todas as abas.
static LOCAL: Mutex<Option<HashMap<String, BTreeMap<String, String>>>> = Mutex::new(None);

thread_local! {
    /// sessionStorage: por aba (cada aba tem a sua thread).
    static SESSION: std::cell::RefCell<HashMap<String, BTreeMap<String, String>>> = Default::default();
}

fn origin(vm: &Vm) -> String {
    read(vm, |h| h.page.url.origin().ascii_serialization())
}

fn with_store<T>(vm: &mut Vm, a: &CallArgs, f: impl FnOnce(&mut BTreeMap<String, String>) -> T) -> T {
    let key = origin(vm);
    if ferro_js::vm::truthy(&a.arg(0)) {
        SESSION.with(|m| f(m.borrow_mut().entry(key).or_default()))
    } else {
        let mut g = LOCAL.lock().unwrap_or_else(|e| e.into_inner());
        f(g.get_or_insert_with(HashMap::new).entry(key).or_default())
    }
}

/// Avisa que o localStorage mudou (para gravar em disco).
fn local_changed(a: &CallArgs) {
    if !ferro_js::vm::truthy(&a.arg(0)) {
        crate::dados::storage_changed();
    }
}

/// O localStorage inteiro, uma linha por chave (origem, chave, valor).
pub fn export_local_storage() -> String {
    use crate::dados::escape;
    let g = LOCAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = String::new();
    for (origin, map) in g.iter().flatten() {
        for (k, v) in map {
            out.push_str(&escape(origin));
            out.push('\t');
            out.push_str(&escape(k));
            out.push('\t');
            out.push_str(&escape(v));
            out.push('\n');
        }
    }
    out
}

/// Lê o que [`export_local_storage`] gravou.
pub fn import_local_storage(text: &str) {
    use crate::dados::unescape;
    let mut g = LOCAL.lock().unwrap_or_else(|e| e.into_inner());
    let all = g.get_or_insert_with(HashMap::new);
    for line in text.lines() {
        let mut f = line.splitn(3, '\t');
        let (Some(o), Some(k), Some(v)) = (f.next(), f.next(), f.next()) else { continue };
        all.entry(unescape(o)).or_default().entry(unescape(k)).or_insert_with(|| unescape(v));
    }
}

fn storage_get(vm: &mut Vm, a: &mut CallArgs) -> R {
    let k = arg_string(vm, a, 1)?;
    let v = with_store(vm, a, |m| m.get(&k).cloned());
    Ok(v.map_or(Value::Null, |v| s(&v)))
}

fn storage_set(vm: &mut Vm, a: &mut CallArgs) -> R {
    let k = arg_string(vm, a, 1)?;
    let v = arg_string(vm, a, 2)?;
    let old = with_store(vm, a, |m| m.insert(k, v.clone()));
    if old.as_deref() != Some(v.as_str()) {
        local_changed(a);
    }
    Ok(Value::Undefined)
}

fn storage_remove(vm: &mut Vm, a: &mut CallArgs) -> R {
    let k = arg_string(vm, a, 1)?;
    if with_store(vm, a, |m| m.remove(&k)).is_some() {
        local_changed(a);
    }
    Ok(Value::Undefined)
}

fn storage_clear(vm: &mut Vm, a: &mut CallArgs) -> R {
    with_store(vm, a, |m| m.clear());
    local_changed(a);
    Ok(Value::Undefined)
}

fn storage_keys(vm: &mut Vm, a: &mut CallArgs) -> R {
    let keys: Vec<String> = with_store(vm, a, |m| m.keys().cloned().collect());
    let items = keys.iter().map(|k| s(k)).collect();
    Ok(vm.array_value(items))
}

// ------------------------------------------------------------------ janela

fn inner_width(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Number(read(vm, |h| h.width) as f64))
}

fn inner_height(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Number(read(vm, |h| h.height) as f64))
}

fn scroll_y(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Number(read(vm, |h| h.scroll_y) as f64))
}

fn zero(_vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Number(0.0))
}

fn one(_vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Number(1.0))
}

fn noop(_vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Undefined)
}

fn request_animation_frame(vm: &mut Vm, a: &mut CallArgs) -> R {
    let cb = a.arg(0);
    if !cb.is_callable() {
        return Err(vm.type_error("Failed to execute 'requestAnimationFrame' on 'Window': The callback provided as parameter 1 is not a function."));
    }
    let id = write(vm, |h| {
        h.raf_seq += 1;
        h.raf.push((h.raf_seq, cb));
        h.raf_seq
    });
    Ok(Value::Number(id as f64))
}

fn cancel_animation_frame(vm: &mut Vm, a: &mut CallArgs) -> R {
    let id = vm.to_number(&a.arg(0))? as u32;
    write(vm, |h| h.raf.retain(|(i, _)| *i != id));
    Ok(Value::Undefined)
}

fn match_media(vm: &mut Vm, a: &mut CallArgs) -> R {
    let q = arg_string(vm, a, 0)?;
    let (width, height) = read(vm, |h| (h.width, h.height));
    Ok(Value::Bool(crate::css::media_matches(&q, &crate::css::MediaContext { width, height })))
}

fn now(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Number(vm.now_ms()))
}

fn report_error(vm: &mut Vm, a: &mut CallArgs) -> R {
    vm.report_uncaught(&a.arg(0));
    Ok(Value::Undefined)
}

/// `__native.log(texto)`: aviso do próprio navegador no console.
fn log(vm: &mut Vm, a: &mut CallArgs) -> R {
    let t = arg_string(vm, a, 0)?;
    eprintln!("[ferro] {t}");
    Ok(Value::Undefined)
}

/// customElements.define: registra e atualiza os elementos que já existem.
fn define_custom(vm: &mut Vm, a: &mut CallArgs) -> R {
    let name = arg_string(vm, a, 0)?.to_ascii_lowercase();
    let Value::Object(ctor) = a.arg(1) else {
        return Err(vm.type_error("Failed to execute 'define' on 'CustomElementRegistry': parameter 2 is not of type 'Function'."));
    };
    if read(vm, |h| h.custom.contains_key(&name)) {
        return Err(super::dom::dom_error(
            vm,
            "NotSupportedError",
            &format!("Failed to execute 'define' on 'CustomElementRegistry': the name \"{name}\" has already been used with this registry"),
        ));
    }
    write(vm, |h| h.custom.insert(name.clone(), ctor));
    // elementos já existentes com essa tag: troca o protótipo e roda o construtor
    let existing: Vec<usize> = read(vm, |h| {
        let doc = &h.page.doc;
        doc.descendants(crate::dom::Document::ROOT).into_iter().filter(|&n| doc.tag(n) == Some(name.as_str())).collect()
    });
    let mut out = Vec::new();
    for n in existing {
        // o mesmo objeto JS: o construtor troca o protótipo dele
        let el = super::wrap(vm, n);
        super::dom::upgrade_custom(vm, n, &el)?;
        out.push(el);
    }
    Ok(vm.array_value(out))
}

