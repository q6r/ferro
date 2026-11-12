//! Módulos ES na página: `<script type=module>` e `import()`.
//!
//! O motor JS transforma cada módulo numa função (ver `ferro_js::module`);
//! aqui ficam o resto: baixar o grafo de dependências pela rede, criar o
//! namespace de cada módulo, e chamar as funções na ordem (dependências
//! antes). Cada URL vira um módulo só por documento.

use std::collections::{HashMap, HashSet};

use ferro_js::value::{CallArgs, ObjRef};
use ferro_js::{Value, Vm};
use url::Url;

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
}

#[derive(Default)]
pub(crate) struct Modules {
    pub recs: HashMap<Url, Rec>,
    pub waiting: Vec<(Url, Waiter)>,
    inline: usize,
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

fn hook(vm: &mut Vm, name: &str, args: Vec<Value>) -> Value {
    let f = read(vm, |h| h.hooks.get(name).cloned());
    match f.map(|f| vm.call(&f, Value::Undefined, args)) {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            vm.report_uncaught(&e);
            Value::Undefined
        }
        None => Value::Undefined,
    }
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
    read(vm, |h| {
        let map = &h.import_map;
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
    let base = read(vm, |h| h.page.base.clone());
    let code = format!("(function(){{ try {{ const m = JSON.parse({}); return Object.entries(m.imports || {{}}).map(e => e[0] + '\\u0000' + e[1]).join('\\u0001'); }} catch (e) {{ return ''; }} }})()", js_string(text));
    let Ok(Value::String(list)) = vm.eval_script(&code, "importmap") else { return };
    let list = list.to_rust();
    write(vm, |h| {
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
    if read(vm, |h| h.modules.recs.contains_key(url)) {
        return;
    }
    let ns = hook(vm, "newNamespace", Vec::new());
    write(vm, |h| {
        h.modules.recs.insert(url.clone(), Rec { state: State::Fetching, ns });
        let net = h.net.clone();
        let u = url.clone();
        h.in_flight += 1;
        h.pool.submit(Box::new(move || {
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
    let (mut url, n) = write(vm, |h| {
        h.modules.inline += 1;
        (h.page.url.clone(), h.modules.inline)
    });
    url.set_fragment(Some(&format!("modulo-{n}")));
    let ns = hook(vm, "newNamespace", Vec::new());
    write(vm, |h| {
        h.modules.recs.insert(url.clone(), Rec { state: State::Fetching, ns });
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
    write(vm, |h| {
        if let Some(r) = h.modules.recs.get_mut(url) {
            r.state = st;
        }
    });
}

/// O grafo a partir de `root` está pronto? None = ainda baixando;
/// Some(Err) = alguma parte falhou.
fn graph_ready(h: &Host, root: &Url) -> Option<Result<(), ()>> {
    let mut seen = HashSet::new();
    let mut stack = vec![root.clone()];
    let mut failed = false;
    while let Some(u) = stack.pop() {
        if !seen.insert(u.clone()) {
            continue;
        }
        match h.modules.recs.get(&u).map(|r| &r.state) {
            None | Some(State::Fetching) => return None,
            Some(State::Failed(_)) | Some(State::Errored(_)) => failed = true,
            Some(State::Ready { deps, .. }) => stack.extend(deps.iter().cloned()),
            Some(State::Evaluating) | Some(State::Evaluated(_)) => {}
        }
    }
    Some(if failed { Err(()) } else { Ok(()) })
}

/// Roda o módulo (e antes as dependências). Devolve a promessa do módulo.
fn evaluate(vm: &mut Vm, url: &Url) -> R {
    let st = write(vm, |h| {
        let r = h.modules.recs.get_mut(url)?;
        Some(match &r.state {
            State::Ready { .. } => std::mem::replace(&mut r.state, State::Evaluating),
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
    let mut args = vec![read(vm, |h| h.modules.recs[url].ns.clone())];
    for d in &deps {
        if let Err(e) = evaluate(vm, d) {
            set_state(vm, url, State::Errored(e.clone()));
            return Err(e);
        }
        args.push(read(vm, |h| h.modules.recs[d].ns.clone()));
    }
    let meta = hook(vm, "importMeta", vec![Value::str(url.as_str())]);
    args.push(meta);
    match vm.call(&func, Value::Undefined, args) {
        Ok(p) => {
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

/// Atende quem estava esperando um grafo que ficou pronto. Devolve se rodou algo.
pub(crate) fn drive(vm: &mut Vm) -> bool {
    let ready: Vec<(Url, Waiter)> = write(vm, |h| {
        let mut out = Vec::new();
        let mut i = 0;
        while i < h.modules.waiting.len() {
            if graph_ready(h, &h.modules.waiting[i].0).is_some() {
                out.push(h.modules.waiting.remove(i));
            } else {
                i += 1;
            }
        }
        out
    });
    let any = !ready.is_empty();
    for (url, w) in ready {
        let result = evaluate(vm, &url);
        let ns = read(vm, |h| h.modules.recs.get(&url).map(|r| r.ns.clone()));
        match w {
            Waiter::Script(n) => {
                let el = wrap(vm, n);
                match &result {
                    Ok(p) => {
                        // erro dentro do módulo (assíncrono) aparece como Uncaught
                        hook(vm, "moduleRan", vec![p.clone()]);
                        if read(vm, |h| h.page.doc.element(n).and_then(|e| e.attr("src")).is_some()) {
                            hook(vm, "fire", vec![el, Value::str("load")]);
                        }
                    }
                    Err(e) => {
                        let fetch_failed = read(vm, |h| {
                            matches!(h.modules.recs.get(&url).map(|r| &r.state), Some(State::Failed(_)))
                        });
                        if fetch_failed {
                            hook(vm, "fire", vec![el, Value::str("error")]);
                        } else {
                            vm.report_uncaught(e);
                        }
                    }
                }
            }
            Waiter::Import { res, rej } => match result {
                Ok(p) => {
                    let done = hook(vm, "moduleDone", vec![p, ns.unwrap_or(Value::Undefined)]);
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
        }
        vm.run_microtasks();
    }
    any
}

/// Começa um `<script type=module>` (do HTML ou inserido por script).
pub(crate) fn start_script(vm: &mut Vm, n: NodeId, src: Option<String>, text: &str) -> Option<Url> {
    let url = match src {
        Some(src) => {
            let u = read(vm, |h| h.page.base.join(src.trim()).ok())?;
            request(vm, &u);
            u
        }
        None => add_inline(vm, text),
    };
    write(vm, |h| h.modules.waiting.push((url.clone(), Waiter::Script(n))));
    Some(url)
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
    let base = read(vm, |h| h.page.base.clone());
    let from = Url::parse(&referrer).ok().filter(|u| matches!(u.scheme(), "http" | "https" | "file")).unwrap_or(base);
    let Some(url) = resolve(vm, &spec, &from) else {
        let e = vm.type_error(format!("Failed to resolve module specifier \"{spec}\""));
        vm.call(&rej, Value::Undefined, vec![e])?;
        return Ok(Value::Undefined);
    };
    request(vm, &url);
    write(vm, |h| h.modules.waiting.push((url, Waiter::Import { res, rej })));
    // já estava tudo carregado: resolve logo
    drive(vm);
    Ok(Value::Undefined)
}

pub(crate) fn install(vm: &Vm, n: &ObjRef) {
    vm.realm.method(n, "importModule", 4, import_module);
}
