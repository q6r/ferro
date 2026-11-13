//! Mensagens entre realms que podem estar em threads diferentes (a página,
//! os quadros, os workers):
//!
//! - cada realm tem uma [`Mailbox`]: a caixa de entrada (o mesmo canal por
//!   onde chegam as respostas da rede), as MessagePort e os BroadcastChannel
//!   abertos nela e os Workers que ela criou;
//! - as MessagePort ficam num registro global: cada porta sabe o par e a
//!   caixa de quem a tem agora (em trânsito, as mensagens esperam);
//! - o BroadcastChannel entrega para todas as realms da mesma origem que
//!   abriram um canal com o mesmo nome.
//!
//! O que atravessa threads é sempre um [`clone::Message`] (dados puros).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use ferro_js::value::{CallArgs, Kind, ObjRef, Object};
use ferro_js::{Value, Vm};
use url::Url;

use super::clone::{self, Message, PORT, Transfer};
use super::worker::WorkerHost;
use super::{Done, Host};
use crate::net::Net;

type R = Result<Value, Value>;

/// Acorda o laço da aba (que dorme esperando a interface).
pub(crate) type Waker = Arc<dyn Fn() + Send + Sync>;

/// Por onde chega trabalho de outras threads a uma realm.
#[derive(Clone)]
pub(crate) struct Inbox {
    tx: mpsc::Sender<Done>,
    /// Mensagens enviadas e ainda não tratadas (o laço não dorme com elas).
    pending: Arc<AtomicUsize>,
    wake: Arc<OnceLock<Waker>>,
}

impl Inbox {
    pub fn new(tx: mpsc::Sender<Done>) -> Inbox {
        Inbox { tx, pending: Arc::new(AtomicUsize::new(0)), wake: Arc::new(OnceLock::new()) }
    }

    /// Entrega; false se a realm já acabou.
    pub fn send(&self, d: Done) -> bool {
        self.pending.fetch_add(1, Ordering::SeqCst);
        if self.tx.send(d).is_err() {
            self.pending.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
        if let Some(w) = self.wake.get() {
            w();
        }
        true
    }

    /// Há mensagens esperando?
    pub fn pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }

    /// Uma mensagem foi tratada.
    pub fn done(&self) {
        let _ = self.pending.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
    }

    pub fn set_waker(&self, w: Waker) {
        let _ = self.wake.set(w);
    }

    pub fn waker(&self) -> Option<Waker> {
        self.wake.get().cloned()
    }

    /// O canal por baixo (para o conjunto de threads de rede da realm).
    pub fn sender(&self) -> mpsc::Sender<Done> {
        self.tx.clone()
    }
}

/// Algo que um worker manda para quem o criou.
pub(crate) enum WorkerEv {
    Message(Message),
    /// Exceção não tratada no worker (já com "Uncaught ...").
    Error { message: String, filename: String, lineno: u32, colno: u32 },
    /// O script do worker não carregou.
    LoadFailed,
    /// A thread do worker está acabando (close() ou fim).
    Closed,
}

/// Um worker visto de quem o criou.
pub(crate) struct WorkerRec {
    /// O objeto `Worker`.
    pub obj: Value,
    /// Caixa de entrada do worker.
    pub inbox: Inbox,
    pub stop: Arc<AtomicBool>,
    pub thread: Option<std::thread::JoinHandle<()>>,
}

/// Depuração: `FERRO_WORKER_LOG=1` mostra os workers criados e o que dá errado neles.
pub(crate) fn log_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FERRO_WORKER_LOG").is_some())
}

/// Workers vivos no processo (limite contra páginas que criam sem parar).
pub(crate) static LIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);
const MAX_WORKERS: usize = 64;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

/// O estado de mensagens de uma realm.
pub(crate) struct Mailbox {
    /// Identidade da realm no processo (portas, canais, blob URLs).
    pub id: u64,
    pub inbox: Inbox,
    /// MessagePorts desta realm (id → objeto).
    pub ports: HashMap<u64, ObjRef>,
    /// `MessagePort.prototype` (para criar as portas que chegam).
    pub port_proto: Option<ObjRef>,
    /// BroadcastChannels abertos aqui (id → objeto).
    pub channels: HashMap<u64, ObjRef>,
    pub workers: HashMap<u32, WorkerRec>,
    pub worker_seq: u32,
    /// Threads de workers parados (para esperar o fim).
    pub dying: Vec<std::thread::JoinHandle<()>>,
}

impl Mailbox {
    pub fn new(tx: mpsc::Sender<Done>) -> Mailbox {
        Mailbox::with_inbox(Inbox::new(tx))
    }

    /// Com uma caixa já criada (a do worker nasce do lado de quem o cria).
    pub fn with_inbox(inbox: Inbox) -> Mailbox {
        Mailbox {
            id: next_id(),
            inbox,
            ports: HashMap::new(),
            port_proto: None,
            channels: HashMap::new(),
            workers: HashMap::new(),
            worker_seq: 0,
            dying: Vec::new(),
        }
    }

    /// Junta as threads de workers que já acabaram.
    pub fn reap(&mut self) {
        let mut i = 0;
        while i < self.dying.len() {
            if self.dying[i].is_finished() {
                let _ = self.dying.swap_remove(i).join();
            } else {
                i += 1;
            }
        }
    }

    fn stop_worker(&mut self, id: u32) {
        if let Some(mut w) = self.workers.remove(&id) {
            w.stop.store(true, Ordering::SeqCst);
            w.inbox.send(Done::Terminate);
            self.dying.extend(w.thread.take());
        }
    }
}

impl Drop for Mailbox {
    /// A realm acabou (navegação, aba fechada, worker no fim): solta as
    /// portas, os canais e os blob URLs e para os workers dela.
    fn drop(&mut self) {
        ports::drop_realm(self.id);
        broadcast::drop_realm(self.id);
        crate::net::revoke_blobs_of(self.id);
        let ids: Vec<u32> = self.workers.keys().copied().collect();
        for id in ids {
            self.stop_worker(id);
        }
        // quem está no meio de JS para na hora; quem espera a rede sai sozinho depois
        let limit = Instant::now() + Duration::from_millis(100);
        for t in self.dying.drain(..) {
            while !t.is_finished() && Instant::now() < limit {
                std::thread::sleep(Duration::from_millis(1));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

/// O que página e worker oferecem às funções daqui.
pub(crate) trait RealmHost {
    fn mailbox(&mut self) -> &mut Mailbox;
    fn hook(&self, name: &str) -> Option<Value>;
    /// Origem serializada ("https://a.com"; "null" se opaca).
    fn origin(&self) -> String;
    /// Endereço do documento ou do script do worker.
    fn url(&self) -> Url;
    /// Base para resolver endereços relativos.
    fn base(&self) -> Url;
    /// Rede presa à origem desta realm.
    fn net(&self) -> Net;
    /// Começa um fetch/XHR; `cb` recebe a resposta.
    fn start_fetch(&mut self, cb: Value, req: crate::cors::PageRequest);
    /// Módulos ES desta realm.
    fn modules(&mut self) -> &mut super::module::Modules;
    /// Import map (`<script type=importmap>`; o worker não tem).
    fn import_map(&self) -> Option<&HashMap<String, Url>>;
    /// Trabalho de rede numa thread do conjunto (conta em `in_flight`).
    fn submit(&mut self, job: super::Job);
}

impl RealmHost for Host {
    fn start_fetch(&mut self, cb: Value, req: crate::cors::PageRequest) {
        super::web::start_fetch(self, cb, req);
    }
    fn modules(&mut self) -> &mut super::module::Modules {
        &mut self.modules
    }
    fn import_map(&self) -> Option<&HashMap<String, Url>> {
        Some(&self.import_map)
    }
    fn submit(&mut self, job: super::Job) {
        self.in_flight += 1;
        self.pool.submit(job);
    }
    fn mailbox(&mut self) -> &mut Mailbox {
        &mut self.mailbox
    }
    fn hook(&self, name: &str) -> Option<Value> {
        self.hooks.get(name).cloned()
    }
    fn origin(&self) -> String {
        super::web::page_origin(self)
    }
    fn url(&self) -> Url {
        self.page.url.clone()
    }
    fn base(&self) -> Url {
        self.page.base.clone()
    }
    fn net(&self) -> Net {
        self.net.clone()
    }
}

/// Roda `f` com o hospedeiro da realm desta VM (página ou worker).
pub(crate) fn realm<T>(vm: &Vm, f: impl FnOnce(&mut dyn RealmHost) -> T) -> T {
    let h = vm.host.clone().expect("VM sem hospedeiro");
    let h = match h.downcast::<RefCell<Host>>() {
        Ok(p) => return f(&mut *p.borrow_mut()),
        Err(h) => h,
    };
    match h.downcast::<RefCell<WorkerHost>>() {
        Ok(w) => f(&mut *w.borrow_mut()),
        Err(_) => panic!("VM sem hospedeiro"),
    }
}

/// Chama um gancho do prelúdio; exceções viram "Uncaught ...".
pub(crate) fn call_hook(vm: &mut Vm, name: &str, args: Vec<Value>) -> Value {
    let Some(f) = realm(vm, |r| r.hook(name)) else { return Value::Undefined };
    match vm.call(&f, Value::Undefined, args) {
        Ok(v) => v,
        Err(e) => {
            vm.report_uncaught(&e);
            Value::Undefined
        }
    }
}

// ------------------------------------------------------------------ portas

mod ports {
    use super::*;

    struct Entry {
        /// O par (0 = desligada).
        partner: u64,
        /// (realm, caixa) de quem tem a porta; None = em trânsito.
        owner: Option<(u64, Inbox)>,
        /// Mensagens que chegaram em trânsito.
        queue: Vec<Message>,
    }

    static PORTS: Mutex<Option<HashMap<u64, Entry>>> = Mutex::new(None);

    fn with<T>(f: impl FnOnce(&mut HashMap<u64, Entry>) -> T) -> T {
        let mut g = PORTS.lock().unwrap_or_else(|e| e.into_inner());
        f(g.get_or_insert_with(HashMap::new))
    }

    /// Duas portas ligadas, as duas na realm `owner`.
    pub fn channel(owner: (u64, Inbox)) -> (u64, u64) {
        let (a, b) = (next_id(), next_id());
        with(|m| {
            m.insert(a, Entry { partner: b, owner: Some(owner.clone()), queue: Vec::new() });
            m.insert(b, Entry { partner: a, owner: Some(owner), queue: Vec::new() });
        });
        (a, b)
    }

    /// Mensagem da porta `from` para o par dela.
    pub fn post(from: u64, msg: Message) {
        with(|m| {
            let Some(to) = m.get(&from).map(|e| e.partner).filter(|&p| p != 0) else { return };
            let Some(e) = m.get_mut(&to) else { return };
            match &e.owner {
                Some((_, inbox)) => {
                    inbox.send(Done::Port { port: to, msg });
                }
                None => e.queue.push(msg),
            }
        });
    }

    /// A porta saiu desta realm (vai dentro de uma mensagem).
    pub fn release(id: u64) {
        with(|m| {
            if let Some(e) = m.get_mut(&id) {
                e.owner = None;
            }
        });
    }

    /// A porta chegou a uma realm: o que esperava vai junto.
    pub fn adopt(id: u64, owner: (u64, Inbox)) {
        with(|m| {
            if let Some(e) = m.get_mut(&id) {
                for msg in e.queue.drain(..) {
                    owner.1.send(Done::Port { port: id, msg });
                }
                e.owner = Some(owner);
            }
        });
    }

    /// `close()`: desliga as duas pontas.
    pub fn close(id: u64) {
        with(|m| {
            if let Some(e) = m.remove(&id)
                && let Some(p) = m.get_mut(&e.partner)
            {
                p.partner = 0;
            }
        });
    }

    /// A realm acabou: as portas dela somem (os pares ficam desligados).
    pub fn drop_realm(realm: u64) {
        with(|m| {
            let gone: Vec<(u64, u64)> = m
                .iter()
                .filter(|(_, e)| e.owner.as_ref().is_some_and(|(r, _)| *r == realm))
                .map(|(&id, e)| (id, e.partner))
                .collect();
            for (id, partner) in gone {
                m.remove(&id);
                if let Some(p) = m.get_mut(&partner) {
                    p.partner = 0;
                }
            }
        });
    }
}

fn port_id(v: &Value) -> Option<u64> {
    match v {
        Value::Object(o) => match o.borrow().kind {
            Kind::Host { class: PORT, id } => Some(id as u64),
            _ => None,
        },
        _ => None,
    }
}

fn new_port(vm: &Vm, id: u64) -> ObjRef {
    let proto = realm(vm, |r| r.mailbox().port_proto.clone()).unwrap_or_else(|| vm.realm.object_proto.clone());
    let mut o = Object::new(Some(proto), Kind::Host { class: PORT, id: id as usize });
    o.tag = "MessagePort";
    let o = ObjRef::new(o);
    realm(vm, |r| r.mailbox().ports.insert(id, o.clone()));
    o
}

/// As portas transferidas saem desta realm: os objetos antigos ficam
/// inúteis (postMessage não faz nada) e o registro marca "em trânsito".
pub(crate) fn release_ports(vm: &mut Vm, t: &Transfer) {
    for (o, id) in &t.ports {
        o.borrow_mut().kind = Kind::Host { class: PORT, id: 0 };
        realm(vm, |r| r.mailbox().ports.remove(id));
        ports::release(*id);
    }
}

/// Portas que chegaram numa mensagem: objetos novos nesta realm.
pub(crate) fn receive_ports(vm: &mut Vm, ids: &[u64]) -> Vec<Value> {
    let owner = realm(vm, |r| {
        let mb = r.mailbox();
        (mb.id, mb.inbox.clone())
    });
    ids.iter()
        .map(|&id| {
            let o = new_port(vm, id);
            ports::adopt(id, owner.clone());
            Value::Object(o)
        })
        .collect()
}

/// `structuredClone` com portas na transferência: elas trocam de objeto.
pub(crate) fn adopt_ports(vm: &mut Vm, t: &Transfer, ids: &[u64]) -> Result<Vec<Value>, Value> {
    release_ports(vm, t);
    Ok(receive_ports(vm, ids))
}

/// Remonta uma mensagem recebida: (dados, portas). Err = messageerror.
pub(crate) fn receive(vm: &mut Vm, msg: Message) -> Result<(Value, Vec<Value>), Value> {
    let ports = receive_ports(vm, &msg.ports);
    let plat_in = realm(vm, |r| r.hook("cloneIn")).unwrap_or(Value::Undefined);
    let data = clone::deserialize(vm, msg, &ports, &plat_in)?;
    Ok((data, ports))
}

/// Lista de transferência já normalizada pelo prelúdio (array ou undefined).
fn list(vm: &mut Vm, v: &Value) -> Result<Vec<Value>, Value> {
    match v {
        Value::Undefined | Value::Null => Ok(Vec::new()),
        v => vm.iterate_to_vec(v),
    }
}

/// `__native.portSetup(MessagePort.prototype)`.
fn port_setup(vm: &mut Vm, a: &mut CallArgs) -> R {
    if let Value::Object(p) = a.arg(0) {
        realm(vm, |r| r.mailbox().port_proto = Some(p));
    }
    Ok(Value::Undefined)
}

/// `__native.channelNew()` → [port1, port2].
fn channel_new(vm: &mut Vm, _a: &mut CallArgs) -> R {
    let owner = realm(vm, |r| {
        let mb = r.mailbox();
        (mb.id, mb.inbox.clone())
    });
    let (a, b) = ports::channel(owner);
    let pa = Value::Object(new_port(vm, a));
    let pb = Value::Object(new_port(vm, b));
    Ok(vm.array_value(vec![pa, pb]))
}

/// `__native.portPost(porta, mensagem, transferência, platOut)`.
fn port_post(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ctx = "Failed to execute 'postMessage' on 'MessagePort': ";
    let src = port_id(&a.arg(0)).unwrap_or(0);
    let items = list(vm, &a.arg(2))?;
    let t = clone::transfer_list(vm, &items, ctx)?;
    if let Some(i) = t.ports.iter().position(|(_, id)| *id == src && src != 0) {
        return Err(clone::clone_error(vm, ctx, &format!("Port at index {i} contains the source port.")));
    }
    let msg = clone::serialize(vm, &a.arg(1), &t, &a.arg(3), ctx)?;
    release_ports(vm, &t);
    if log_enabled() {
        eprintln!("[worker] porta {src}: {}", msg.value.summary());
    }
    if src != 0 {
        ports::post(src, msg);
    }
    Ok(Value::Undefined)
}

/// `__native.portClose(porta)`.
fn port_close(vm: &mut Vm, a: &mut CallArgs) -> R {
    if let Some(id) = port_id(&a.arg(0)).filter(|&id| id != 0) {
        if let Value::Object(o) = a.arg(0) {
            o.borrow_mut().kind = Kind::Host { class: PORT, id: 0 };
        }
        realm(vm, |r| r.mailbox().ports.remove(&id));
        ports::close(id);
    }
    Ok(Value::Undefined)
}

// --------------------------------------------------------- BroadcastChannel

mod broadcast {
    use super::*;

    pub struct Sub {
        pub id: u64,
        pub realm: u64,
        pub origin: String,
        pub name: String,
        pub inbox: Inbox,
    }

    static SUBS: Mutex<Vec<Sub>> = Mutex::new(Vec::new());

    pub fn with<T>(f: impl FnOnce(&mut Vec<Sub>) -> T) -> T {
        f(&mut SUBS.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn drop_realm(realm: u64) {
        with(|s| s.retain(|x| x.realm != realm));
    }
}

/// `__native.bcOpen(canal, nome)` → id.
fn bc_open(vm: &mut Vm, a: &mut CallArgs) -> R {
    let Value::Object(o) = a.arg(0) else { return Ok(Value::Undefined) };
    let name = vm.to_rust_string(&a.arg(1))?;
    let id = next_id();
    let (realm_id, origin, inbox) = realm(vm, |r| {
        let origin = r.origin();
        let mb = r.mailbox();
        mb.channels.insert(id, o);
        (mb.id, origin, mb.inbox.clone())
    });
    broadcast::with(|s| s.push(broadcast::Sub { id, realm: realm_id, origin, name, inbox }));
    Ok(Value::Number(id as f64))
}

/// `__native.bcPost(id, mensagem, platOut)`.
fn bc_post(vm: &mut Vm, a: &mut CallArgs) -> R {
    let id = vm.to_number(&a.arg(0))? as u64;
    let ctx = "Failed to execute 'postMessage' on 'BroadcastChannel': ";
    let t = Transfer { buffers: Vec::new(), ports: Vec::new() };
    let msg = clone::serialize(vm, &a.arg(1), &t, &a.arg(2), ctx)?;
    broadcast::with(|s| {
        let Some((origin, name)) = s.iter().find(|x| x.id == id).map(|x| (x.origin.clone(), x.name.clone())) else { return };
        for x in s.iter().filter(|x| x.id != id && x.origin == origin && x.name == name) {
            x.inbox.send(Done::Broadcast { id: x.id, msg: msg.clone() });
        }
    });
    Ok(Value::Undefined)
}

/// `__native.bcClose(id)`.
fn bc_close(vm: &mut Vm, a: &mut CallArgs) -> R {
    let id = vm.to_number(&a.arg(0))? as u64;
    broadcast::with(|s| s.retain(|x| x.id != id));
    realm(vm, |r| r.mailbox().channels.remove(&id));
    Ok(Value::Undefined)
}

// ----------------------------------------------------------------- workers

/// `__native.workerNew(objeto, url, tipo, nome)` → id.
fn worker_new(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ctx = "Failed to construct 'Worker': ";
    let input = vm.to_rust_string(&a.arg(1))?;
    let module = vm.to_rust_string(&a.arg(2))? == "module";
    let name = vm.to_rust_string(&a.arg(3))?;
    let (base, origin, page, net) = realm(vm, |r| (r.base(), r.origin(), r.url(), r.net()));
    let Ok(url) = base.join(input.trim()) else {
        return Err(super::dom::dom_error(vm, "SyntaxError", &format!("{ctx}'{input}' is not a valid URL.")));
    };
    // o script precisa ser da mesma origem (data: ganha origem opaca)
    let allowed = match url.scheme() {
        "data" => true,
        "http" | "https" | "blob" => url.origin().ascii_serialization() == origin,
        "file" => page.scheme() == "file",
        _ => false,
    };
    if !allowed {
        return Err(super::dom::dom_error(
            vm,
            "SecurityError",
            &format!("{ctx}Script at '{url}' cannot be accessed from origin '{origin}'."),
        ));
    }
    if log_enabled() {
        let shown: String = url.as_str().chars().take(120).collect();
        eprintln!("[worker] novo{}: {shown}", if module { " (módulo)" } else { "" });
    }
    let worker_origin = match url.scheme() {
        "http" | "https" | "blob" => url.origin().ascii_serialization(),
        _ => "null".to_string(),
    };
    let (id, parent) = realm(vm, |r| {
        let mb = r.mailbox();
        mb.reap();
        mb.worker_seq += 1;
        (mb.worker_seq, mb.inbox.clone())
    });
    // blob: vale o conteúdo de agora (é comum revogar logo depois do new Worker)
    let script = match url.scheme() {
        "blob" => crate::net::blob(url.as_str(), Some(&origin)).map(|(mime, bytes)| crate::net::decode_text(&bytes, Some(&mime), false)),
        _ => None,
    };
    let (tx, rx) = mpsc::channel();
    let inbox = Inbox::new(tx);
    let stop = Arc::new(AtomicBool::new(false));
    let blob_gone = url.scheme() == "blob" && script.is_none();
    let thread = if LIVE_WORKERS.load(Ordering::SeqCst) < MAX_WORKERS && !blob_gone {
        super::worker::spawn(super::worker::Params {
            id,
            url,
            module,
            name,
            origin: worker_origin,
            net,
            script,
            parent: parent.clone(),
            inbox: inbox.clone(),
            rx,
            stop: stop.clone(),
        })
    } else {
        None
    };
    if thread.is_none() {
        // não deu para criar a thread: o worker "não carregou"
        parent.send(Done::Worker { id, ev: WorkerEv::LoadFailed });
    }
    realm(vm, |r| r.mailbox().workers.insert(id, WorkerRec { obj: a.arg(0), inbox, stop, thread }));
    Ok(Value::Number(id as f64))
}

/// `__native.workerPost(id, mensagem, transferência, platOut)`.
fn worker_post(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ctx = "Failed to execute 'postMessage' on 'Worker': ";
    let id = vm.to_number(&a.arg(0))? as u32;
    let items = list(vm, &a.arg(2))?;
    let t = clone::transfer_list(vm, &items, ctx)?;
    let msg = clone::serialize(vm, &a.arg(1), &t, &a.arg(3), ctx)?;
    release_ports(vm, &t);
    if log_enabled() {
        eprintln!("[worker] para o worker {id}: {}", msg.value.summary());
    }
    let inbox = realm(vm, |r| r.mailbox().workers.get(&id).map(|w| w.inbox.clone()));
    if let Some(inbox) = inbox {
        inbox.send(Done::Parent(msg));
    }
    Ok(Value::Undefined)
}

/// `__native.workerTerminate(id)`.
fn worker_terminate(vm: &mut Vm, a: &mut CallArgs) -> R {
    let id = vm.to_number(&a.arg(0))? as u32;
    realm(vm, |r| r.mailbox().stop_worker(id));
    Ok(Value::Undefined)
}

/// `__native.blobRegister(url, bytes, tipo)`: `URL.createObjectURL(blob)`.
fn blob_register(vm: &mut Vm, a: &mut CallArgs) -> R {
    let url = vm.to_rust_string(&a.arg(0))?;
    let bytes = super::web::bytes_of(&a.arg(1)).unwrap_or_default();
    let mime = vm.to_rust_string(&a.arg(2))?;
    let owner = realm(vm, |r| r.mailbox().id);
    crate::net::register_blob(&url, owner, &mime, bytes);
    Ok(Value::Undefined)
}

/// `__native.blobRevoke(url)`.
fn blob_revoke(vm: &mut Vm, a: &mut CallArgs) -> R {
    let url = vm.to_rust_string(&a.arg(0))?;
    crate::net::revoke_blob(&url);
    Ok(Value::Undefined)
}

/// `__native.blobGet(url)` → [bytes, tipo] ou null (fetch de blob:).
fn blob_get(vm: &mut Vm, a: &mut CallArgs) -> R {
    let url = vm.to_rust_string(&a.arg(0))?;
    let origin = realm(vm, |r| r.origin());
    let Some((mime, bytes)) = crate::net::blob(&url, Some(&origin)) else { return Ok(Value::Null) };
    let b = super::web::uint8_array(vm, &bytes)?;
    Ok(vm.array_value(vec![b, Value::str(&mime)]))
}

// ----------------------------------------------------------------- entrega

/// Trata o que chegou pela caixa de entrada: mensagens de portas, de
/// workers criados aqui e de canais. (O que é só do worker, como a mensagem
/// de quem o criou, fica em `worker.rs`.)
pub(crate) fn deliver(vm: &mut Vm, d: Done) {
    match d {
        Done::Port { port, msg } => {
            let Some(target) = realm(vm, |r| r.mailbox().ports.get(&port).cloned()) else { return };
            let target = Value::Object(target);
            match receive(vm, msg) {
                Ok((data, ports)) => {
                    let ports = vm.array_value(ports);
                    call_hook(vm, "portMessage", vec![target, data, ports]);
                }
                Err(_) => {
                    call_hook(vm, "portMessage", vec![target, Value::Undefined, Value::Null]);
                }
            }
        }
        Done::Broadcast { id, msg } => {
            let Some(target) = realm(vm, |r| r.mailbox().channels.get(&id).cloned()) else { return };
            let target = Value::Object(target);
            let origin = realm(vm, |r| r.origin());
            match receive(vm, msg) {
                Ok((data, _)) => {
                    call_hook(vm, "broadcastMessage", vec![target, data, Value::str(&origin)]);
                }
                Err(_) => {
                    call_hook(vm, "broadcastMessage", vec![target, Value::Undefined, Value::Null]);
                }
            }
        }
        Done::Worker { id, ev } => {
            let Some(obj) = realm(vm, |r| r.mailbox().workers.get(&id).map(|w| w.obj.clone())) else { return };
            match ev {
                WorkerEv::Message(msg) => match receive(vm, msg) {
                    Ok((data, ports)) => {
                        let ports = vm.array_value(ports);
                        call_hook(vm, "workerEvent", vec![obj, Value::str("message"), data, ports]);
                    }
                    Err(_) => {
                        call_hook(vm, "workerEvent", vec![obj, Value::str("messageerror")]);
                    }
                },
                WorkerEv::Error { message, filename, lineno, colno } => {
                    let args = vec![
                        obj,
                        Value::str("error"),
                        Value::str(&message),
                        Value::str(&filename),
                        Value::Number(lineno as f64),
                        Value::Number(colno as f64),
                    ];
                    // ninguém cancelou: vai para o console, como no Chrome
                    if ferro_js::vm::truthy(&call_hook(vm, "workerEvent", args)) {
                        (vm.output)(ferro_js::Out::Error, &message);
                    }
                }
                WorkerEv::LoadFailed => {
                    call_hook(vm, "workerEvent", vec![obj, Value::str("loaderror")]);
                }
                WorkerEv::Closed => {
                    realm(vm, |r| {
                        let mb = r.mailbox();
                        if let Some(mut w) = mb.workers.remove(&id) {
                            mb.dying.extend(w.thread.take());
                        }
                    });
                }
            }
        }
        _ => {}
    }
}

/// Funções nativas comuns a página e worker.
pub(crate) fn install(vm: &Vm, n: &ObjRef) {
    let r = &vm.realm;
    for (name, len, f) in [
        ("structuredClone", 5, clone::structured_clone as ferro_js::value::NativeFn),
        ("portSetup", 1, port_setup),
        ("channelNew", 0, channel_new),
        ("portPost", 4, port_post),
        ("portClose", 1, port_close),
        ("bcOpen", 2, bc_open),
        ("bcPost", 3, bc_post),
        ("bcClose", 1, bc_close),
        ("workerNew", 4, worker_new),
        ("workerPost", 4, worker_post),
        ("workerTerminate", 1, worker_terminate),
        ("blobRegister", 3, blob_register),
        ("blobRevoke", 1, blob_revoke),
        ("blobGet", 1, blob_get),
    ] {
        r.method(n, name, len, f);
    }
}
