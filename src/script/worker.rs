//! Web Workers dedicados (`new Worker(url, { type, name })`).
//!
//! Cada worker roda numa thread própria, com VM, realm e laço de eventos
//! próprios: timers, microtarefas, rede (o mesmo [`Pool`] da página) e o que
//! chega pela caixa de entrada (mensagens de quem criou, de portas, de
//! canais). Quem criou fica com um [`messaging::WorkerRec`]; os dados vão e
//! voltam pelo clone estruturado.
//!
//! O global é um `DedicatedWorkerGlobalScope`, sem DOM: `comum.js` (eventos,
//! fetch/XHR, URL, Blob, crypto, portas...) + `worker.js` (self, location,
//! navigator, importScripts, postMessage, close).
//!
//! `terminate()` liga a bandeira de parada da VM (o JS para no próximo laço
//! ou chamada) e acorda o laço; `close()` termina depois da tarefa atual.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferro_js::builtins::{native_data, set_native_data};
use ferro_js::value::{CallArgs, Kind, ObjRef, Object, PropKey};
use ferro_js::{Engine, Out, Value, Vm};
use url::Url;

use super::clone::{self, Message};
use super::messaging::{self, Inbox, LIVE_WORKERS, Mailbox, RealmHost, WorkerEv, call_hook};
use super::module::Modules;
use super::{Done, Pool};
use crate::net::{self, Net};

type R = Result<Value, Value>;

/// Prelúdio dos workers: a parte comum (também das páginas) e a do worker.
const PRELUDE: &str = concat!("(function (N) {\n'use strict';\n", include_str!("comum.js"), include_str!("worker.js"), "\n})(__native);\n");

/// O que a thread nova recebe de quem cria o worker.
pub(crate) struct Params {
    pub id: u32,
    pub url: Url,
    pub module: bool,
    pub name: String,
    /// Origem do worker ("null" para data:).
    pub origin: String,
    /// Rede de quem criou (vira a do worker, presa ao endereço dele).
    pub net: Net,
    /// O texto do script, se já veio (blob: é lido na criação).
    pub script: Option<String>,
    /// Caixa de quem criou.
    pub parent: Inbox,
    /// A caixa do próprio worker (quem criou tem uma cópia).
    pub inbox: Inbox,
    pub rx: mpsc::Receiver<Done>,
    pub stop: Arc<AtomicBool>,
}

/// Tudo o que o JS de um worker enxerga do lado do Rust.
pub(crate) struct WorkerHost {
    id: u32,
    /// `location` (o endereço final do script).
    url: Url,
    origin: String,
    name: String,
    module: bool,
    net: Net,
    pub(crate) mailbox: Mailbox,
    parent: Inbox,
    hooks: HashMap<&'static str, Value>,
    pool: Pool,
    in_flight: usize,
    fetch_seq: u32,
    fetch_waiters: HashMap<u32, Value>,
    modules: Modules,
    /// O script principal já rodou: as mensagens de quem criou passam a
    /// ser entregues (antes disso esperam em `queued`).
    started: bool,
    queued: VecDeque<Message>,
    /// `close()` foi chamado: acaba depois da tarefa atual.
    closing: bool,
    /// Tratando uma exceção (um erro dentro do `onerror` só vai ao console).
    reporting: bool,
}

impl RealmHost for WorkerHost {
    fn mailbox(&mut self) -> &mut Mailbox {
        &mut self.mailbox
    }
    fn hook(&self, name: &str) -> Option<Value> {
        self.hooks.get(name).cloned()
    }
    fn origin(&self) -> String {
        self.origin.clone()
    }
    fn url(&self) -> Url {
        self.url.clone()
    }
    fn base(&self) -> Url {
        self.url.clone()
    }
    fn net(&self) -> Net {
        self.net.clone()
    }
    fn start_fetch(&mut self, cb: Value, req: crate::cors::PageRequest) {
        self.fetch_seq += 1;
        let id = self.fetch_seq;
        self.fetch_waiters.insert(id, cb);
        let net = self.net.clone();
        self.submit(Box::new(move || Done::Fetch { id, res: crate::cors::fetch(&net, req) }));
    }
    fn modules(&mut self) -> &mut Modules {
        &mut self.modules
    }
    fn import_map(&self) -> Option<&HashMap<String, Url>> {
        None
    }
    fn submit(&mut self, job: super::Job) {
        self.in_flight += 1;
        self.pool.submit(job);
    }
}

fn worker_host(vm: &Vm) -> Rc<RefCell<WorkerHost>> {
    vm.host
        .clone()
        .and_then(|h| h.downcast::<RefCell<WorkerHost>>().ok())
        .expect("VM sem worker")
}

/// Cria a thread do worker (None se o sistema não deixou).
pub(crate) fn spawn(p: Params) -> Option<std::thread::JoinHandle<()>> {
    /// Conta os workers vivos enquanto a thread existe.
    struct Live;
    impl Drop for Live {
        fn drop(&mut self) {
            LIVE_WORKERS.fetch_sub(1, Ordering::SeqCst);
        }
    }
    LIVE_WORKERS.fetch_add(1, Ordering::SeqCst);
    let live = Live;
    std::thread::Builder::new()
        .name("ferro-worker".into())
        .stack_size(super::STACK_SIZE)
        .spawn(move || {
            let _live = live;
            run(p);
        })
        .ok()
}

fn run(p: Params) {
    let Params { id, url, module, name, origin, net, script, parent, inbox, rx, stop } = p;
    let net = net.for_document(&url);
    let pool = Pool::new(inbox.sender());
    let host = Rc::new(RefCell::new(WorkerHost {
        id,
        url: url.clone(),
        origin,
        name,
        module,
        net,
        mailbox: Mailbox::with_inbox(inbox),
        parent: parent.clone(),
        hooks: HashMap::new(),
        pool,
        in_flight: 0,
        fetch_seq: 0,
        fetch_waiters: HashMap::new(),
        modules: Default::default(),
        started: false,
        queued: VecDeque::new(),
        closing: false,
        reporting: false,
    }));
    let mut js = Engine::new();
    js.vm.max_depth = 3000;
    crate::jshost::setup(&mut js);
    let quiet = stop.clone();
    js.set_output(move |kind, text| {
        // parado por terminate(): o que sobrar não interessa
        if quiet.load(Ordering::Relaxed) {
            return;
        }
        match kind {
            Out::Log => eprintln!("[console] {text}"),
            Out::Error => eprintln!("[console:erro] {text}"),
        }
    });
    js.vm.interrupt = Some(stop.clone());
    js.vm.on_uncaught = Some(on_uncaught);
    js.vm.host = Some(host.clone() as Rc<dyn std::any::Any>);
    install(&mut js.vm);
    if let Err(e) = js.eval(PRELUDE, "ferro:worker") {
        eprintln!("[ferro] erro no prelúdio do worker: {}", js.error_text(&e));
    }
    let hooks = super::take_hooks(&mut js.vm);
    host.borrow_mut().hooks = hooks;
    if !stop.load(Ordering::SeqCst) {
        if module {
            super::module::start_worker(&mut js.vm, &url, script);
        } else {
            run_classic(&mut js.vm, &url, script);
        }
        event_loop(&mut js, &host, &rx, &stop);
    }
    // fim: avisa quem criou e solta tudo (portas, canais, workers filhos)
    parent.send(Done::Worker { id, ev: WorkerEv::Closed });
    drop(js);
    drop(host);
    // os ciclos da realm que acabou (a thread vai embora com a memória limpa)
    ferro_js::gc::collect();
}

/// Baixa o script (na thread do worker: pode esperar). (endereço final, texto)
fn load_script(net: &Net, url: &Url) -> Option<(Url, String)> {
    let res = net.request(&net::Request::get(url)).ok()?;
    if !(200..300).contains(&res.status) {
        return None;
    }
    // o fragmento não vai na rede, mas fica no endereço (location.hash)
    let mut final_url = res.url.clone();
    if final_url.fragment().is_none() {
        final_url.set_fragment(url.fragment());
    }
    Some((final_url, net::decode_text(&res.body, res.content_type.as_deref(), false)))
}

fn run_classic(vm: &mut Vm, url: &Url, script: Option<String>) {
    let net = worker_host(vm).borrow().net.clone();
    let Some((final_url, text)) = script.map(|t| (url.clone(), t)).or_else(|| load_script(&net, url)) else {
        load_failed(vm);
        return;
    };
    worker_host(vm).borrow_mut().url = final_url.clone();
    if let Err(e) = vm.eval_script(&text, final_url.as_str()) {
        vm.report_uncaught(&e);
    }
    vm.run_microtasks();
    worker_host(vm).borrow_mut().started = true;
}

/// O script não carregou: `error` (Event simples) no Worker e fim.
fn load_failed(vm: &mut Vm) {
    let h = worker_host(vm);
    let mut b = h.borrow_mut();
    if messaging::log_enabled() {
        eprintln!("[worker] não carregou: {}", b.url);
    }
    b.parent.send(Done::Worker { id: b.id, ev: WorkerEv::LoadFailed });
    b.closing = true;
}

/// O módulo principal de um worker de módulo rodou (ou falhou). Num worker
/// de módulo, qualquer falha (rede, sintaxe, exceção no corpo) chega ao
/// Worker como um `error` simples (Event), como no Chrome; o erro vai ao console.
pub(crate) fn main_module_done(vm: &mut Vm, result: Result<Value, Value>, fetch_failed: bool) {
    match result {
        // o corpo é assíncrono: um erro nele rejeita a promessa (gancho moduleRan)
        Ok(_) => worker_host(vm).borrow_mut().started = true,
        Err(_) if fetch_failed => load_failed(vm),
        Err(e) => {
            module_failed(vm, &e);
            worker_host(vm).borrow_mut().closing = true;
        }
    }
}

/// Erro de um worker de módulo: no console e `error` (Event) no Worker.
fn module_failed(vm: &mut Vm, e: &Value) {
    if vm.interrupted() {
        return;
    }
    let (message, _, _, _) = error_info(vm, e);
    (vm.output)(Out::Error, &message);
    let h = worker_host(vm);
    let b = h.borrow();
    b.parent.send(Done::Worker { id: b.id, ev: WorkerEv::LoadFailed });
}

/// `__native.moduleFailed(erro)`: o corpo do módulo principal rejeitou.
fn module_failed_native(vm: &mut Vm, a: &mut CallArgs) -> R {
    module_failed(vm, &a.arg(0));
    Ok(Value::Undefined)
}

fn event_loop(js: &mut Engine, host: &Rc<RefCell<WorkerHost>>, rx: &mpsc::Receiver<Done>, stop: &AtomicBool) {
    let done = |host: &Rc<RefCell<WorkerHost>>| stop.load(Ordering::SeqCst) || host.borrow().closing;
    loop {
        if done(host) {
            return;
        }
        // timers vencidos (com limite, para as mensagens não esperarem demais);
        // antes das mensagens, como no Chrome (um setTimeout 0 do script
        // roda antes das mensagens que esperavam o script terminar)
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(50) && js.vm.run_one_timer(false) {
            if done(host) {
                return;
            }
        }
        // mensagens de quem criou que esperavam o script rodar
        while host.borrow().started {
            let Some(msg) = host.borrow_mut().queued.pop_front() else { break };
            deliver_parent(&mut js.vm, msg);
            if done(host) {
                return;
            }
        }
        let mut got: Vec<Done> = rx.try_iter().collect();
        if got.is_empty() {
            // dorme até o próximo timer ou até chegar algo
            let now = js.vm.now_ms();
            let next = js.vm.timers.iter().map(|t| t.due).fold(f64::INFINITY, f64::min);
            let r = if next.is_finite() {
                let ms = (next - now).max(0.0);
                if ms > 0.0 { Some(rx.recv_timeout(Duration::from_secs_f64(ms / 1000.0))) } else { None }
            } else {
                Some(rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected))
            };
            match r {
                Some(Ok(d)) => got.push(d),
                Some(Err(mpsc::RecvTimeoutError::Disconnected)) => return,
                _ => {}
            }
        }
        for d in got {
            handle(js, host, d);
            if done(host) {
                return;
            }
        }
        if ferro_js::gc::should_collect() {
            ferro_js::gc::collect();
        }
        host.borrow_mut().mailbox.reap();
    }
}

fn handle(js: &mut Engine, host: &Rc<RefCell<WorkerHost>>, d: Done) {
    {
        let mut b = host.borrow_mut();
        if d.via_inbox() {
            b.mailbox.inbox.done();
        } else {
            b.in_flight = b.in_flight.saturating_sub(1);
        }
    }
    let vm = &mut js.vm;
    match d {
        Done::Parent(msg) => {
            let started = host.borrow().started;
            if started {
                deliver_parent(vm, msg);
            } else {
                host.borrow_mut().queued.push_back(msg);
            }
        }
        Done::Terminate => {}
        Done::Fetch { id, res } => {
            let cb = host.borrow_mut().fetch_waiters.remove(&id);
            if let Some(cb) = cb {
                let arg = super::web::response_value(vm, res);
                if let Err(e) = vm.call(&cb, Value::Undefined, vec![arg]) {
                    vm.report_uncaught(&e);
                }
            }
        }
        Done::Module { url, text } => {
            super::module::arrived(vm, url, text);
            super::module::drive(vm);
        }
        d @ (Done::Port { .. } | Done::Broadcast { .. } | Done::Worker { .. }) => messaging::deliver(vm, d),
        _ => {}
    }
    vm.run_microtasks();
}

/// Mensagem de quem criou: evento `message` (ou `messageerror`) no global.
fn deliver_parent(vm: &mut Vm, msg: Message) {
    match messaging::receive(vm, msg) {
        Ok((data, ports)) => {
            let ports = vm.array_value(ports);
            call_hook(vm, "scopeMessage", vec![data, ports]);
        }
        Err(_) => {
            call_hook(vm, "scopeMessage", vec![Value::Undefined, Value::Null]);
        }
    }
    vm.run_microtasks();
}

/// Exceção não tratada no worker: `error` no global e, se ninguém
/// cancelar, no objeto `Worker` de quem criou (que decide se vai ao console).
fn on_uncaught(vm: &mut Vm, e: &Value) -> bool {
    if vm.interrupted() {
        return true;
    }
    let h = worker_host(vm);
    if h.borrow().reporting {
        return false;
    }
    h.borrow_mut().reporting = true;
    let (message, filename, lineno, colno) = error_info(vm, e);
    let has_hook = h.borrow().hooks.contains_key("scopeError");
    let go_on = !has_hook || {
        let args = vec![
            Value::str(&message),
            Value::str(&filename),
            Value::Number(lineno as f64),
            Value::Number(colno as f64),
            e.clone(),
        ];
        ferro_js::vm::truthy(&call_hook(vm, "scopeError", args))
    };
    let mut b = h.borrow_mut();
    if messaging::log_enabled() {
        eprintln!("[worker] {message} ({filename}:{lineno})");
    }
    if go_on {
        b.parent.send(Done::Worker { id: b.id, ev: WorkerEv::Error { message, filename, lineno, colno } });
    }
    b.reporting = false;
    true
}

/// (mensagem com "Uncaught ...", arquivo, linha, coluna) de uma exceção.
fn error_info(vm: &mut Vm, e: &Value) -> (String, String, u32, u32) {
    let mut file = worker_host(vm).borrow().url.to_string();
    let mut line = 0;
    let is_error = matches!(e, Value::Object(o) if matches!(o.borrow().kind, Kind::Error));
    let mut text = match e {
        Value::Object(_) if !is_error => vm.describe(e),
        _ => vm.to_rust_string(e).unwrap_or_else(|_| vm.describe(e)),
    };
    if let Value::Object(o) = e {
        let stack = o.borrow().props.get(&PropKey::str("stack")).and_then(|p| match &p.slot {
            ferro_js::value::Slot::Data { value: Value::String(s), .. } => Some(s.to_rust()),
            _ => None,
        });
        // primeira chamada da pilha: "    at f (arquivo:linha)" ou "    at arquivo:linha"
        if let Some(at) = stack.as_deref().and_then(|s| s.lines().find_map(|l| l.trim_start().strip_prefix("at "))) {
            let place = at.rsplit_once(" (").map_or(at, |(_, p)| p.trim_end_matches(')'));
            if let Some((f, l)) = place.rsplit_once(':')
                && let Ok(l) = l.parse()
            {
                file = f.to_string();
                line = l;
            }
        }
    }
    // erro de sintaxe do analisador: "mensagem (arquivo:linha)"
    if line == 0
        && let Some(open) = text.rfind(" (")
        && text.ends_with(')')
        && let Some((f, l)) = text[open + 2..text.len() - 1].rsplit_once(':')
        && let Ok(l) = l.parse()
    {
        file = f.to_string();
        line = l;
        text.truncate(open);
    }
    (format!("Uncaught {text}"), file, line, if line > 0 { 1 } else { 0 })
}

// ------------------------------------------------------------- nativas

fn illegal(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Err(vm.type_error("Illegal constructor"))
}

fn event_target_ctor(vm: &mut Vm, a: &mut CallArgs) -> R {
    let def = match native_data(&a.callee) {
        Value::Object(p) => p,
        _ => vm.realm.object_proto.clone(),
    };
    let proto = vm.proto_from_ctor(a.new_target.as_ref(), &def)?;
    Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::Ordinary))))
}

/// `__native.href()`: `location.href` do worker.
fn href(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::str(worker_host(vm).borrow().url.as_str()))
}

/// `__native.workerInfo()` → [nome, é módulo].
fn worker_info(vm: &mut Vm, _a: &mut CallArgs) -> R {
    let (name, module) = {
        let h = worker_host(vm);
        let b = h.borrow();
        (b.name.clone(), b.module)
    };
    Ok(vm.array_value(vec![Value::str(&name), Value::Bool(module)]))
}

/// `__native.scopePost(mensagem, transferência, platOut)`: `self.postMessage`.
fn scope_post(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ctx = "Failed to execute 'postMessage' on 'DedicatedWorkerGlobalScope': ";
    let items = match a.arg(1) {
        Value::Undefined | Value::Null => Vec::new(),
        v => vm.iterate_to_vec(&v)?,
    };
    let t = clone::transfer_list(vm, &items, ctx)?;
    let msg = clone::serialize(vm, &a.arg(0), &t, &a.arg(2), ctx)?;
    messaging::release_ports(vm, &t);
    let h = worker_host(vm);
    let b = h.borrow();
    if messaging::log_enabled() {
        eprintln!("[worker] do worker {}: {}", b.id, msg.value.summary());
    }
    b.parent.send(Done::Worker { id: b.id, ev: WorkerEv::Message(msg) });
    Ok(Value::Undefined)
}

/// `__native.scopeClose()`: `self.close()`.
fn scope_close(vm: &mut Vm, _a: &mut CallArgs) -> R {
    worker_host(vm).borrow_mut().closing = true;
    Ok(Value::Undefined)
}

/// `__native.importScripts([urls])`: baixa e roda cada um, em ordem.
fn import_scripts(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ctx = "Failed to execute 'importScripts' on 'WorkerGlobalScope': ";
    let (base, net, module) = {
        let h = worker_host(vm);
        let b = h.borrow();
        (b.url.clone(), b.net.clone(), b.module)
    };
    if module {
        return Err(vm.type_error(format!("{ctx}Module scripts don't support importScripts().")));
    }
    let items = vm.iterate_to_vec(&a.arg(0))?;
    let mut urls = Vec::with_capacity(items.len());
    for it in &items {
        let s = vm.to_rust_string(it)?;
        match base.join(s.trim()) {
            Ok(u) => urls.push(u),
            Err(_) => return Err(super::dom::dom_error(vm, "SyntaxError", &format!("{ctx}The URL '{s}' is invalid."))),
        }
    }
    for u in urls {
        let Some((final_url, text)) = load_script(&net, &u) else {
            return Err(super::dom::dom_error(vm, "NetworkError", &format!("{ctx}The script at '{u}' failed to load.")));
        };
        vm.eval_script(&text, final_url.as_str())?;
    }
    Ok(Value::Undefined)
}

/// Protótipo com construtor global, na cadeia de `parent`.
fn make_proto(vm: &mut Vm, name: &'static str, parent: &ObjRef, ctor: ferro_js::value::NativeFn) -> ObjRef {
    let p = ObjRef::new(Object::new(Some(parent.clone()), Kind::Ordinary));
    vm.realm.tag(&p, name);
    let c = vm.realm.ctor(name, 0, ctor, &p);
    set_native_data(&c, Value::Object(p.clone()));
    // a cadeia dos construtores segue a herança (WorkerGlobalScope.__proto__ === EventTarget)
    if !parent.ptr_eq(&vm.realm.object_proto)
        && let Some(Value::Object(pc)) = parent.borrow().props.get(&vm.realm.key_constructor).and_then(|pr| match &pr.slot {
            ferro_js::value::Slot::Data { value, .. } => Some(value.clone()),
            _ => None,
        })
    {
        c.borrow_mut().proto = Some(pc);
    }
    p
}

/// O global do worker e as funções nativas que o prelúdio usa.
fn install(vm: &mut Vm) {
    let object_proto = vm.realm.object_proto.clone();
    let et = make_proto(vm, "EventTarget", &object_proto, event_target_ctor);
    let wgs = make_proto(vm, "WorkerGlobalScope", &et, illegal);
    let dwgs = make_proto(vm, "DedicatedWorkerGlobalScope", &wgs, illegal);
    vm.realm.global.borrow_mut().proto = Some(dwgs);
    let n = vm.new_object();
    super::web::install_common(vm, &n);
    super::module::install(vm, &n);
    for (name, len, f) in [
        ("href", 0, href as ferro_js::value::NativeFn),
        ("workerInfo", 0, worker_info),
        ("scopePost", 3, scope_post),
        ("scopeClose", 0, scope_close),
        ("importScripts", 1, import_scripts),
        ("moduleFailed", 1, module_failed_native),
    ] {
        vm.realm.method(&n, name, len, f);
    }
    vm.realm.global_value("__native", Value::Object(n));
}
