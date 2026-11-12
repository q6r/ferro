//! Coletor de ciclos.
//!
//! Os objetos do Ferro JS são contados por referência (`Rc`): o que fica
//! sem dono é liberado na hora. O que escapa são os **ciclos** (um objeto
//! que aponta para outro que aponta de volta, uma função cujo ambiente
//! guarda a própria função...). Este módulo acha e desfaz esses ciclos.
//!
//! Algoritmo ("trial deletion", de Bacon e Rajan): para cada objeto e
//! ambiente vivos, parte da contagem de referências e desconta as que vêm
//! de dentro do próprio heap. Quem fica com saldo positivo é segurado por
//! alguém de fora (a pilha da VM, o Rust do navegador, variáveis globais):
//! é raiz. Tudo o que se alcança a partir das raízes está vivo; o resto só
//! é apontado por lixo — esvaziamos esses objetos e o `Rc` libera o ciclo.
//!
//! Não precisa conhecer as raízes. É seguro mesmo se alguma referência
//! interna não for enumerada (o alvo apenas parece segurado de fora e fica).
//! Valores de WeakMap são efêmeros: só ficam vivos se a chave estiver viva.
//!
//! Cada objeto/ambiente/closure guarda no campo `gc` a época da coleta e a
//! sua posição nela (assim as arestas viram índices sem tabela de hash).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use crate::builtins::promise::PState;
use crate::value::{Closure, Env, Func, Kind, MapKey, ObjRef, Object, PropMap, Scope, Slot, Value, WeakKey};
use crate::vm::Frame;

thread_local! {
    static OBJECTS: RefCell<Vec<Weak<RefCell<Object>>>> = const { RefCell::new(Vec::new()) };
    static ENVS: RefCell<Vec<Weak<RefCell<Scope>>>> = const { RefCell::new(Vec::new()) };
    /// Alocações desde a última coleta.
    static SINCE: Cell<usize> = const { Cell::new(0) };
    /// Vivos depois da última coleta.
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static EPOCH: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn track_object(o: &Rc<RefCell<Object>>) {
    if !ENABLED.with(Cell::get) {
        return;
    }
    OBJECTS.with(|l| l.borrow_mut().push(Rc::downgrade(o)));
    SINCE.with(|s| s.set(s.get() + 1));
}

pub(crate) fn track_env(e: &Rc<RefCell<Scope>>) {
    if !ENABLED.with(Cell::get) {
        return;
    }
    ENVS.with(|l| l.borrow_mut().push(Rc::downgrade(e)));
    SINCE.with(|s| s.set(s.get() + 1));
}

/// Liga/desliga o registro (desligado, nada novo é coletado).
pub fn set_enabled(on: bool) {
    ENABLED.with(|e| e.set(on));
}

/// Coleta só se for urgente (o usuário está interagindo: pausa atrapalha).
pub fn should_collect_urgent() -> bool {
    let since = SINCE.with(Cell::get);
    since > 1_000_000 && since > 4 * LIVE.with(Cell::get) && should_collect()
}

/// Vale a pena coletar agora? (Muitas alocações desde a última vez.)
pub fn should_collect() -> bool {
    thread_local!(static OFF: bool = std::env::var_os("FERRO_SEM_GC").is_some());
    if OFF.with(|o| *o) {
        return false;
    }
    let since = SINCE.with(Cell::get);
    // o trabalho de uma coleta é proporcional ao que está vivo: coletando a
    // cada 2× isso em alocações, o custo por alocação fica constante
    since > 300_000 && since > 2 * LIVE.with(Cell::get)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// Objetos, ambientes e closures examinados.
    pub examined: usize,
    /// Objetos e ambientes em ciclos de lixo (esvaziados).
    pub freed: usize,
    pub ms: f64,
}

enum Node {
    Obj(Rc<RefCell<Object>>),
    Env(Rc<RefCell<Scope>>),
    Clo(Rc<Closure>),
}

/// Estado de uma coleta: nós, arestas (CSR) e a época.
struct Graph {
    epoch: u64,
    nodes: Vec<Node>,
    busy: Vec<bool>,
    edge_start: Vec<u32>,
    edges: Vec<u32>,
    /// (mapa, chave: None = símbolo/fora do registro, Some(MAX) = morta, valor)
    ephemerons: Vec<(u32, Option<u32>, u32)>,
    weak_colls: Vec<u32>,
}

impl Graph {
    fn tag(&self, idx: usize) -> u64 {
        self.epoch << 32 | idx as u64
    }

    fn idx(&self, tag: u64) -> Option<u32> {
        (tag >> 32 == self.epoch).then_some(tag as u32)
    }

    fn obj(&self, o: &ObjRef) -> Option<u32> {
        o.0.try_borrow().ok().and_then(|b| self.idx(b.gc.get()))
    }

    fn env(&self, e: &Env) -> Option<u32> {
        e.try_borrow().ok().and_then(|b| self.idx(b.gc.get()))
    }

    /// Closure: ganha um nó na primeira vez que aparece.
    fn clo(&mut self, c: &Rc<Closure>) -> u32 {
        if let Some(i) = self.idx(c.gc.get()) {
            return i;
        }
        let i = self.nodes.len();
        c.gc.set(self.tag(i));
        self.nodes.push(Node::Clo(c.clone()));
        i as u32
    }

    fn value(&self, v: &Value, out: &mut Vec<u32>) {
        if let Value::Object(o) = v
            && let Some(i) = self.obj(o)
        {
            out.push(i);
        }
    }

    fn objref(&self, o: &ObjRef, out: &mut Vec<u32>) {
        if let Some(i) = self.obj(o) {
            out.push(i);
        }
    }

    fn frame(&self, f: &Frame, out: &mut Vec<u32>) {
        for v in f.regs.iter().chain(&f.stack).chain(&f.args) {
            self.value(v, out);
        }
        if let Some(e) = &f.env
            && let Some(i) = self.env(e)
        {
            out.push(i);
        }
        self.value(&f.this, out);
        self.value(&f.new_target, out);
        for o in [&f.func, &f.home].into_iter().flatten() {
            self.objref(o, out);
        }
    }

    /// Referências fortes de um objeto (e os efêmeros, se for WeakMap/WeakSet).
    fn object(&mut self, me: u32, o: &Object, out: &mut Vec<u32>) {
        if let Some(p) = &o.proto {
            self.objref(p, out);
        }
        for (_, prop) in o.props.iter() {
            match &prop.slot {
                Slot::Data { value, .. } => self.value(value, out),
                Slot::Accessor { get, set } => {
                    for x in [get, set].into_iter().flatten() {
                        self.objref(x, out);
                    }
                }
            }
        }
        match &o.kind {
            Kind::Array(items) => items.iter().for_each(|x| self.value(x, out)),
            Kind::Function(Func::Closure(c)) => {
                let i = self.clo(c);
                out.push(i);
            }
            Kind::Function(Func::Native(n)) => self.value(&n.data, out),
            Kind::Function(Func::Bound { target, this, args }) => {
                self.objref(target, out);
                self.value(this, out);
                args.iter().for_each(|x| self.value(x, out));
            }
            Kind::Map(c) | Kind::Set(c) => {
                for (k, val) in c.entries.iter().flatten() {
                    self.value(k, out);
                    self.value(val, out);
                }
                for k in c.index.keys() {
                    if let MapKey::Object(o) = k {
                        self.objref(o, out);
                    }
                }
            }
            Kind::WeakMap(c) | Kind::WeakSet(c) => {
                self.weak_colls.push(me);
                for (key, val) in c.map.values() {
                    let Value::Object(v) = val else { continue };
                    let Some(vi) = self.obj(v) else { continue };
                    let ki = match key {
                        WeakKey::Obj(w) => match w.upgrade() {
                            None => Some(u32::MAX),
                            Some(k) => k.try_borrow().ok().and_then(|b| self.idx(b.gc.get())),
                        },
                        WeakKey::Sym(_) => None,
                    };
                    self.ephemerons.push((me, ki, vi));
                }
            }
            Kind::ArrayIter { target, .. } => self.value(target, out),
            Kind::MapIter { target, .. } => self.objref(target, out),
            Kind::ForIn { target: Some(t), .. } => self.objref(t, out),
            Kind::IterRecord { iter, next, .. } => {
                self.value(iter, out);
                self.value(next, out);
            }
            Kind::RegExpStringIter { rx, .. } => self.objref(rx, out),
            Kind::Generator(g) => {
                if let Some(f) = &g.frame {
                    self.frame(f, out);
                }
            }
            Kind::Promise(p) => {
                if let PState::Fulfilled(x) | PState::Rejected(x) = &p.state {
                    self.value(x, out);
                }
                for r in p.fulfill.iter().chain(&p.reject) {
                    self.value(&r.handler, out);
                    if let Some(c) = &r.cap {
                        self.value(&c.promise, out);
                        self.value(&c.resolve, out);
                        self.value(&c.reject, out);
                    }
                }
            }
            Kind::PromiseResolver { promise, .. } => self.objref(promise, out),
            Kind::Async(a) => {
                if let Some(f) = &a.frame {
                    self.frame(f, out);
                }
                self.objref(&a.promise, out);
            }
            Kind::Proxy { target, handler } => {
                for x in [target, handler].into_iter().flatten() {
                    self.objref(x, out);
                }
            }
            Kind::TypedArray(t) | Kind::DataView(t) => self.objref(&t.buffer, out),
            Kind::AsyncGenerator(a) => {
                if let Some(f) = &a.frame {
                    self.frame(f, out);
                }
                for r in &a.queue {
                    self.value(&r.value, out);
                    self.value(&r.cap.promise, out);
                    self.value(&r.cap.resolve, out);
                    self.value(&r.cap.reject, out);
                }
            }
            _ => {}
        }
    }

