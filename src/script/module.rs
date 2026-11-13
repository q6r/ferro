//! Módulos ES na página e nos workers: `<script type=module>`, workers de
//! módulo (`new Worker(url, { type: 'module' })`) e `import()`.
//!
//! O sistema de módulos é o do motor (`ferro_js::module`: carga do grafo,
//! ligação, avaliação, namespaces); aqui ficam só a resolução (import map),
//! a busca pela rede e o que fazer quando um módulo principal termina.

use ferro_js::module::{ModuleHost, Ran};
use ferro_js::value::{CallArgs, ObjRef, PropKey, Property};
use ferro_js::{JsStr, Value, Vm};
use url::Url;

use super::messaging::{call_hook, realm};
use super::{Done, Host, host, wrap};
use crate::dom::NodeId;

/// Estado dos módulos que é da página (o resto mora no motor).
#[derive(Default)]
pub(crate) struct Modules {
    /// `<script type=module>` cujo grafo ainda não rodou.
    running: Vec<NodeId>,
    inline: usize,
}

/// Leitura do estado da página (só para o que é dela: `<script>`, import map).
fn page<T>(vm: &Vm, f: impl FnOnce(&mut Host) -> T) -> T {
    let h = host(vm);
    let mut b = h.borrow_mut();
    f(&mut b)
}

/// Resolve um especificador (`./a.js`, `/x.js`, `https://...`) contra a URL
/// de quem importa. Especificadores "nus" (`lodash`) passam pelo import map.
pub(crate) fn resolve(vm: &Vm, spec: &str, referrer: &Url) -> Option<Url> {
    let spec = spec.trim();
    if spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') {
        return referrer.join(spec).ok();
    }
    if let Ok(u) = Url::parse(spec) {
        return Some(u);
    }
    // import map: { "imports": { "nome": "url", "prefixo/": "url/" } }
    realm(vm, |r| {
        let map = r.import_map()?;
        if let Some(u) = map.get(spec) {
            return Some(u.clone());
        }
        map.iter()
            .filter(|(k, _)| k.ends_with('/') && spec.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .and_then(|(k, base)| base.join(&spec[k.len()..]).ok())
    })
}

/// Resolve relativo a quem importa (URL http/https/file; senão a base da realm).
fn resolve_from(vm: &Vm, spec: &str, referrer: &str) -> Result<Url, String> {
    let from = Url::parse(referrer)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https" | "file"))
        .unwrap_or_else(|| realm(vm, |r| r.base()));
    resolve(vm, spec, &from).ok_or_else(|| {
        format!("Failed to resolve module specifier \"{spec}\". Relative references must start with either \"/\", \"./\", or \"../\".")
    })
}

/// Lê um `<script type=importmap>` (só a parte "imports").
pub(crate) fn add_import_map(vm: &mut Vm, text: &str) {
    let base = page(vm, |h| h.page.base.clone());
    let code = format!("(function(){{ try {{ const m = JSON.parse({}); return Object.entries(m.imports || {{}}).map(e => e[0] + '\\u0000' + e[1]).join('\\u0001'); }} catch (e) {{ return ''; }} }})()", js_string(text));
    let Ok(Value::String(list)) = vm.eval_script(&code, "importmap") else { return };
    let list = list.to_rust();
    page(vm, |h| {
        for pair in list.split('\u{1}') {
            if let Some((k, v)) = pair.split_once('\u{0}')
                && let Ok(u) = base.join(v)
            {
                h.import_map.insert(k.to_string(), u);
            }
        }
    });
}

fn js_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ------------------------------------------------------------ ganchos do motor

fn host_resolve(vm: &mut Vm, spec: &str, referrer: &str) -> Result<String, String> {
    resolve_from(vm, spec, referrer).map(String::from)
}

/// Busca pela rede (numa thread do conjunto); o texto volta em `Done::Module`.
fn host_fetch(vm: &mut Vm, url: &str) -> Option<ferro_js::module::Fetched> {
    let Ok(u) = Url::parse(url) else {
        return Some(Err(format!("Failed to fetch module: {url}")));
    };
    realm(vm, |r| {
        let net = r.net();
        r.submit(Box::new(move || {
            let bytes = match net.fetch(&u) {
                Ok(r) if r.status < 400 => Some(r.body),
                _ => None,
            };
            Done::Module { url: u, bytes }
        }));
    });
    None
}

/// `import.meta`: `url` e `resolve(especificador)` (com o import map).
fn host_meta(vm: &mut Vm, url: &str, meta: &ObjRef) {
    let resolve = vm.realm.native("resolve", 1, meta_resolve);
    ferro_js::builtins::set_native_data(&resolve, Value::str(url));
    let mut b = meta.borrow_mut();
    b.props.insert(PropKey::str("url"), Property::data(Value::str(url)));
    b.props.insert(PropKey::str("resolve"), Property::data(Value::Object(resolve)));
}

fn meta_resolve(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let spec = vm.to_string(&a.arg(0))?.to_rust();
    let referrer = match ferro_js::builtins::native_data(&a.callee) {
        Value::String(s) => s.to_rust(),
        _ => String::new(),
    };
    match resolve_from(vm, &spec, &referrer) {
        Ok(u) => Ok(Value::String(JsStr::new(u.as_str()))),
        Err(msg) => Err(vm.type_error(msg)),
    }
}

/// Liga o sistema de módulos do motor a esta realm (página ou worker).
pub(crate) fn install(vm: &mut Vm) {
    vm.modules.host = Some(ModuleHost {
        resolve: host_resolve,
        fetch: host_fetch,
        meta: host_meta,
    });
}

/// O texto de um módulo chegou (ou a busca falhou).
pub(crate) fn fetched(vm: &mut Vm, url: Url, bytes: Option<Vec<u8>>) {
    let r = bytes.ok_or_else(|| format!("Failed to fetch module: {url}"));
    vm.module_fetched(url.as_str(), r);
    vm.run_microtasks();
}

// ------------------------------------------------------------ módulos principais

/// Começa a baixar o módulo (e as dependências) antes da hora de rodar.
pub(crate) fn prefetch(vm: &mut Vm, url: &Url) {
    vm.prefetch_module(url.as_str());
}

/// Começa um `<script type=module>` (do HTML ou inserido por script).
pub(crate) fn start_script(vm: &mut Vm, n: NodeId, src: Option<String>, text: &str) -> Option<Url> {
    let external = src.is_some();
    let (url, text) = match src {
        Some(src) => (page(vm, |h| h.page.base.join(src.trim()).ok())?, None),
        None => {
            // inline: ganha uma URL própria (a do documento com um fragmento)
            let (mut url, n) = page(vm, |h| {
                h.modules.inline += 1;
                (h.page.url.clone(), h.modules.inline)
            });
            url.set_fragment(Some(&format!("modulo-{n}")));
            (url, Some(text.to_string()))
        }
    };
    page(vm, |h| h.modules.running.push(n));
    vm.run_module(url.as_str(), text, Box::new(move |vm, r| script_ran(vm, n, external, r)));
    Some(url)
}

/// O grafo de um `<script type=module>` rodou (ou falhou): eventos do elemento.
fn script_ran(vm: &mut Vm, n: NodeId, external: bool, r: Ran) {
    page(vm, |h| h.modules.running.retain(|x| *x != n));
    let el = wrap(vm, n);
    match r {
        // erro no corpo: a promessa rejeita (aparece como Uncaught (in promise))
        Ran::Started(_) => {
            if external {
                call_hook(vm, "fire", vec![el, Value::str("load")]);
            }
        }
        Ran::FetchFailed(_) => {
            call_hook(vm, "fire", vec![el, Value::str("error")]);
        }
        Ran::Failed(e) => vm.report_uncaught(&e),
    }
    vm.run_microtasks();
}

/// Começa o módulo principal de um worker (`text`: já veio, como no blob:).
pub(crate) fn start_worker(vm: &mut Vm, url: &Url, text: Option<String>) {
    vm.run_module(
        url.as_str(),
        text,
        Box::new(|vm, r| {
            match r {
                Ran::Started(p) => {
                    // o corpo é assíncrono: um erro nele rejeita a promessa
                    // (o gancho a trata antes das microtarefas)
                    call_hook(vm, "moduleRan", vec![p.clone()]);
                    super::worker::main_module_done(vm, Ok(p), false);
                }
                Ran::FetchFailed(e) => super::worker::main_module_done(vm, Err(e), true),
                Ran::Failed(e) => super::worker::main_module_done(vm, Err(e), false),
            }
            vm.run_microtasks();
        }),
    );
}

/// O grafo desse `<script type=module>` já rodou (ou falhou)?
pub(crate) fn script_done(h: &Host, n: NodeId) -> bool {
    !h.modules.running.contains(&n)
}
