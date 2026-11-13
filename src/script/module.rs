//! Módulos ES na página e nos workers: `<script type=module>`, workers de
//! módulo (`new Worker(url, { type: 'module' })`) e `import()`.
//!
//! O motor JS transforma cada módulo numa função (ver `ferro_js::module`);
//! aqui ficam o resto: baixar o grafo de dependências pela rede, criar o
//! namespace de cada módulo, e chamar as funções na ordem (dependências
//! antes). Cada URL vira um módulo só por realm.

use std::collections::{HashMap, HashSet};

use ferro_js::value::{CallArgs, ObjRef};
use ferro_js::{Value, Vm};
use url::Url;

use super::messaging::{call_hook, realm};
use super::{Done, Host, host, wrap};
use crate::dom::NodeId;

type R = Result<Value, Value>;

pub(crate) enum State {
    /// Baixando o texto.
    Fetching,
    /// Compilado; esperando as dependências / a vez de rodar.
    Ready { func: Value, deps: Vec<Url> },
    /// Não deu para baixar ou resolver um especificador.
    Failed(String),
    /// Rodando (num ciclo, quem chega de novo não espera).
    Evaluating,
    /// Já rodou: a promessa da função assíncrona do módulo.
    Evaluated(Value),
    /// Erro de sintaxe ou exceção ao rodar.
    Errored(Value),
}

pub(crate) struct Rec {
    pub state: State,
    /// O namespace (o objeto que quem importa enxerga).
    pub ns: Value,
}

/// Quem espera um grafo de módulos ficar pronto.
pub(crate) enum Waiter {
    /// `<script type=module>`: roda e dispara load/error no elemento.
    Script(NodeId),
    /// `import()`: resolve ou rejeita a promessa.
    Import { res: Value, rej: Value },
    /// O script principal de um worker de módulo.
    Worker,
}

#[derive(Default)]
pub(crate) struct Modules {
    pub recs: HashMap<Url, Rec>,
    pub waiting: Vec<(Url, Waiter)>,
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

/// Garante que o módulo dessa URL existe (pede à rede se for novo).
pub(crate) fn request(vm: &mut Vm, url: &Url) {
    if realm(vm, |r| r.modules().recs.contains_key(url)) {
        return;
    }
    let ns = call_hook(vm, "newNamespace", Vec::new());
    realm(vm, |r| {
        r.modules().recs.insert(url.clone(), Rec { state: State::Fetching, ns });
        let net = r.net();
        let u = url.clone();
        r.submit(Box::new(move || {
            let text = match net.fetch(&u) {
                Ok(r) if r.status < 400 => Some(crate::net::decode_text(&r.body, r.content_type.as_deref(), false)),
                _ => None,
            };
            Done::Module { url: u, text }
        }));
    });
}

/// Um módulo inline (`<script type=module>texto</script>`): ganha uma URL
/// própria (a do documento com um fragmento) e é compilado na hora.
pub(crate) fn add_inline(vm: &mut Vm, text: &str) -> Url {
    let (mut url, n) = page(vm, |h| {
        h.modules.inline += 1;
        (h.page.url.clone(), h.modules.inline)
    });
    url.set_fragment(Some(&format!("modulo-{n}")));
    let ns = call_hook(vm, "newNamespace", Vec::new());
    realm(vm, |r| {
        r.modules().recs.insert(url.clone(), Rec { state: State::Fetching, ns });
    });
    arrived(vm, url.clone(), Some(text.to_string()));
    url
}

/// O texto de um módulo chegou: compila e pede as dependências.
pub(crate) fn arrived(vm: &mut Vm, url: Url, text: Option<String>) {
    let Some(text) = text else {
        set_state(vm, &url, State::Failed(format!("Failed to fetch module: {url}")));
        return;
    };
    match ferro_js::module::module_function(vm, &text, url.as_str()) {
        Ok((requests, func)) => {
            let mut deps = Vec::new();
            for r in &requests {
                match resolve(vm, r, &url) {
                    Some(u) => deps.push(u),
                    None => {
                        let msg = format!(
                            "Failed to resolve module specifier \"{r}\". Relative references must start with either \"/\", \"./\", or \"../\"."
                        );
                        set_state(vm, &url, State::Failed(msg));
                        return;
                    }
                }
            }
            for d in &deps {
                request(vm, d);
            }
            set_state(vm, &url, State::Ready { func, deps });
        }
        Err(e) => set_state(vm, &url, State::Errored(e)),
    }
}

fn set_state(vm: &Vm, url: &Url, st: State) {
    realm(vm, |r| {
        if let Some(rec) = r.modules().recs.get_mut(url) {
            rec.state = st;
        }
    });
}

/// O grafo a partir de `root` está pronto? None = ainda baixando;
/// Some(Err) = alguma parte falhou.
fn graph_ready(m: &Modules, root: &Url) -> Option<Result<(), ()>> {
    let mut seen = HashSet::new();
    let mut stack = vec![root.clone()];
    let mut failed = false;
    while let Some(u) = stack.pop() {
        if !seen.insert(u.clone()) {
            continue;
        }
        match m.recs.get(&u).map(|r| &r.state) {
            None | Some(State::Fetching) => return None,
            Some(State::Failed(_)) | Some(State::Errored(_)) => failed = true,
            Some(State::Ready { deps, .. }) => stack.extend(deps.iter().cloned()),
            Some(State::Evaluating) | Some(State::Evaluated(_)) => {}
        }
    }
    Some(if failed { Err(()) } else { Ok(()) })
}

/// Roda o módulo (e antes as dependências). Devolve a promessa do módulo.
/// `root`: o módulo principal de um worker (a promessa vai ao gancho
/// `moduleRan` antes das microtarefas, para um erro nele não virar
/// "Uncaught (in promise)").
fn evaluate(vm: &mut Vm, url: &Url, root: bool) -> R {
    let st = realm(vm, |r| {
        let rec = r.modules().recs.get_mut(url)?;
        Some(match &rec.state {
            State::Ready { .. } => std::mem::replace(&mut rec.state, State::Evaluating),
            State::Evaluated(p) => State::Evaluated(p.clone()),
            State::Errored(e) => State::Errored(e.clone()),
            State::Failed(m) => State::Failed(m.clone()),
            State::Evaluating => State::Evaluating,
            State::Fetching => State::Fetching,
        })
    });
    let (func, deps) = match st {
        Some(State::Ready { func, deps }) => (func, deps),
        Some(State::Evaluated(p)) => return Ok(p),
        Some(State::Errored(e)) => return Err(e),
        Some(State::Failed(m)) => return Err(vm.type_error(m)),
        Some(State::Evaluating) => return Ok(Value::Undefined),
        _ => return Err(vm.type_error(format!("módulo não carregado: {url}"))),
    };
    let ns_of = |vm: &Vm, u: &Url| realm(vm, |r| r.modules().recs.get(u).map(|x| x.ns.clone()).unwrap_or(Value::Undefined));
    let mut args = vec![ns_of(vm, url)];
    for d in &deps {
        if let Err(e) = evaluate(vm, d, false) {
            set_state(vm, url, State::Errored(e.clone()));
            return Err(e);
        }
        args.push(ns_of(vm, d));
    }
    let meta = call_hook(vm, "importMeta", vec![Value::str(url.as_str())]);
    args.push(meta);
    match vm.call(&func, Value::Undefined, args) {
        Ok(p) => {
            if root {
                call_hook(vm, "moduleRan", vec![p.clone()]);
            }
            vm.run_microtasks();
            set_state(vm, url, State::Evaluated(p.clone()));
            Ok(p)
        }
        Err(e) => {
            set_state(vm, url, State::Errored(e.clone()));
            Err(e)
        }
    }
}

/// O módulo dessa URL não chegou pela rede (ou um especificador falhou)?
fn fetch_failed(vm: &Vm, url: &Url) -> bool {
    realm(vm, |r| matches!(r.modules().recs.get(url).map(|x| &x.state), Some(State::Failed(_))))
}

/// Atende quem estava esperando um grafo que ficou pronto. Devolve se rodou algo.
pub(crate) fn drive(vm: &mut Vm) -> bool {
    let ready: Vec<(Url, Waiter)> = realm(vm, |r| {
        let m = r.modules();
        let mut out = Vec::new();
        let mut i = 0;
        while i < m.waiting.len() {
            if graph_ready(m, &m.waiting[i].0).is_some() {
                out.push(m.waiting.remove(i));
            } else {
                i += 1;
            }
        }
        out
    });
    let any = !ready.is_empty();
    for (url, w) in ready {
        let result = evaluate(vm, &url, matches!(w, Waiter::Worker));
        let ns = realm(vm, |r| r.modules().recs.get(&url).map(|x| x.ns.clone()));
        match w {
            Waiter::Script(n) => {
                let el = wrap(vm, n);
                match &result {
                    Ok(p) => {
                        // erro dentro do módulo (assíncrono) aparece como Uncaught
                        call_hook(vm, "moduleRan", vec![p.clone()]);
                        if page(vm, |h| h.page.doc.element(n).and_then(|e| e.attr("src")).is_some()) {
                            call_hook(vm, "fire", vec![el, Value::str("load")]);
                        }
                    }
                    Err(e) => {
                        if fetch_failed(vm, &url) {
                            call_hook(vm, "fire", vec![el, Value::str("error")]);
                        } else {
                            vm.report_uncaught(e);
                        }
                    }
                }
            }
            Waiter::Import { res, rej } => match result {
                Ok(p) => {
                    let done = call_hook(vm, "moduleDone", vec![p, ns.unwrap_or(Value::Undefined)]);
                    if let Err(e) = vm.call(&res, Value::Undefined, vec![done]) {
                        vm.report_uncaught(&e);
                    }
                }
                Err(e) => {
                    if let Err(e) = vm.call(&rej, Value::Undefined, vec![e]) {
                        vm.report_uncaught(&e);
                    }
                }
            },
            Waiter::Worker => {
                let failed = result.is_err() && fetch_failed(vm, &url);
                super::worker::main_module_done(vm, result, failed);
            }
        }
        vm.run_microtasks();
    }
    any
}

/// Começa um `<script type=module>` (do HTML ou inserido por script).
pub(crate) fn start_script(vm: &mut Vm, n: NodeId, src: Option<String>, text: &str) -> Option<Url> {
    let url = match src {
        Some(src) => {
            let u = page(vm, |h| h.page.base.join(src.trim()).ok())?;
            request(vm, &u);
            u
        }
        None => add_inline(vm, text),
    };
    page(vm, |h| h.modules.waiting.push((url.clone(), Waiter::Script(n))));
    Some(url)
}

/// Começa o módulo principal de um worker (`text`: já veio, como no blob:).
pub(crate) fn start_worker(vm: &mut Vm, url: &Url, text: Option<String>) {
    match text {
        Some(t) => {
            let ns = call_hook(vm, "newNamespace", Vec::new());
            realm(vm, |r| r.modules().recs.insert(url.clone(), Rec { state: State::Fetching, ns }));
            arrived(vm, url.clone(), Some(t));
        }
        None => request(vm, url),
    }
    realm(vm, |r| r.modules().waiting.push((url.clone(), Waiter::Worker)));
    drive(vm);
}

/// O grafo desse `<script type=module>` já rodou (ou falhou)?
pub(crate) fn script_done(h: &Host, n: NodeId) -> bool {
    !h.modules.waiting.iter().any(|(_, w)| matches!(w, Waiter::Script(x) if *x == n))
}

/// `__native.importModule(especificador, quem importa, resolve, reject)`.
fn import_module(vm: &mut Vm, a: &mut CallArgs) -> R {
    let spec = vm.to_string(&a.arg(0))?.to_rust();
    let referrer = vm.to_string(&a.arg(1))?.to_rust();
    let (res, rej) = (a.arg(2), a.arg(3));
    let base = realm(vm, |r| r.base());
    let from = Url::parse(&referrer).ok().filter(|u| matches!(u.scheme(), "http" | "https" | "file")).unwrap_or(base);
    let Some(url) = resolve(vm, &spec, &from) else {
        let e = vm.type_error(format!("Failed to resolve module specifier \"{spec}\""));
        vm.call(&rej, Value::Undefined, vec![e])?;
        return Ok(Value::Undefined);
    };
    request(vm, &url);
    realm(vm, |r| r.modules().waiting.push((url, Waiter::Import { res, rej })));
    // já estava tudo carregado: resolve logo
    drive(vm);
    Ok(Value::Undefined)
}

pub(crate) fn install(vm: &Vm, n: &ObjRef) {
    vm.realm.method(n, "importModule", 4, import_module);
}
