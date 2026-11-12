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

    fn closure(&self, c: &Closure, out: &mut Vec<u32>) {
        if let Some(e) = &c.env
            && let Some(i) = self.env(e)
        {
            out.push(i);
        }
        for x in [&c.this, &c.new_target].into_iter().flatten() {
            self.value(x, out);
        }
        for o in [&c.home, &c.fields].into_iter().flatten() {
            self.objref(o, out);
        }
    }

    /// Marca a partir de `start` (já marcado).
    fn mark_from(&self, start: u32, live: &mut [bool], stack: &mut Vec<u32>) {
        stack.push(start);
        while let Some(x) = stack.pop() {
            let x = x as usize;
            for &j in &self.edges[self.edge_start[x] as usize..self.edge_start[x + 1] as usize] {
                if !live[j as usize] {
                    live[j as usize] = true;
                    stack.push(j);
                }
            }
        }
    }
}

/// Procura e desfaz os ciclos de lixo. Rode entre tarefas (com a pilha da
/// VM vazia o resultado é melhor, mas é seguro em qualquer momento).
pub fn collect() -> Stats {
    let t = std::time::Instant::now();
    let epoch = EPOCH.with(|e| {
        e.set(e.get() + 1);
        e.get()
    });
    let mut g = Graph {
        epoch,
        nodes: Vec::new(),
        busy: Vec::new(),
        edge_start: Vec::new(),
        edges: Vec::new(),
        ephemerons: Vec::new(),
        weak_colls: Vec::new(),
    };
    // 1. nós: objetos e ambientes vivos (seguramos uma referência de cada)
    OBJECTS.with(|l| {
        let mut l = l.borrow_mut();
        l.retain(|w| w.strong_count() > 0);
        g.nodes.reserve(l.len() * 5 / 4);
        for w in l.iter() {
            if let Some(o) = w.upgrade() {
                if let Ok(b) = o.try_borrow() {
                    b.gc.set(epoch << 32 | g.nodes.len() as u64);
                }
                g.nodes.push(Node::Obj(o));
            }
        }
    });
    ENVS.with(|l| {
        let mut l = l.borrow_mut();
        l.retain(|w| w.strong_count() > 0);
        for w in l.iter() {
            if let Some(e) = w.upgrade() {
                if let Ok(b) = e.try_borrow() {
                    b.gc.set(epoch << 32 | g.nodes.len() as u64);
                }
                g.nodes.push(Node::Env(e));
            }
        }
    });
    // 2. arestas (as closures entram como nós ao aparecer)
    let mut out: Vec<u32> = Vec::new();
    g.edge_start.reserve(g.nodes.len() + 1);
    let mut i = 0;
    while i < g.nodes.len() {
        out.clear();
        let mut busy = false;
        match &g.nodes[i] {
            Node::Obj(o) => {
                let o = o.clone();
                match o.try_borrow() {
                    Ok(b) => g.object(i as u32, &b, &mut out),
                    Err(_) => busy = true,
                }
            }
            Node::Env(e) => match e.try_borrow() {
                Ok(b) => {
                    if let Some(p) = &b.parent
                        && let Some(j) = g.env(p)
                    {
                        out.push(j);
                    }
                    for s in &b.slots {
                        g.value(s, &mut out);
                    }
                }
                Err(_) => busy = true,
            },
            Node::Clo(c) => g.closure(c, &mut out),
        }
        g.edge_start.push(g.edges.len() as u32);
        g.edges.extend_from_slice(&out);
        g.busy.push(busy);
        i += 1;
    }
    g.edge_start.push(g.edges.len() as u32);
    let n = g.nodes.len();
    // 3. saldo: referências fortes menos as que vêm de dentro (e menos a nossa)
    let mut count: Vec<i64> = g
        .nodes
        .iter()
        .map(|nd| {
            (match nd {
                Node::Obj(o) => Rc::strong_count(o),
                Node::Env(e) => Rc::strong_count(e),
                Node::Clo(c) => Rc::strong_count(c),
            }) as i64
                - 1
        })
        .collect();
    for &j in &g.edges {
        count[j as usize] -= 1;
    }
    for &(_, _, v) in &g.ephemerons {
        count[v as usize] -= 1;
    }
    // 4. marca o que as raízes alcançam
    let mut live = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    for k in 0..n {
        if (count[k] > 0 || g.busy[k]) && !live[k] {
            live[k] = true;
            g.mark_from(k as u32, &mut live, &mut stack);
        }
    }
    // efêmeros: o valor vive se o mapa e a chave vivem (até estabilizar)
    loop {
        let mut changed = false;
        for &(m, key, v) in &g.ephemerons {
            let key_live = match key {
                None => true,
                Some(u32::MAX) => false,
                Some(k) => live[k as usize],
            };
            if live[m as usize] && key_live && !live[v as usize] {
                live[v as usize] = true;
                changed = true;
                g.mark_from(v, &mut live, &mut stack);
            }
        }
        if !changed {
            break;
        }
    }
    if std::env::var_os("FERRO_GC_DEBUG").is_some() {
        let mut hist: std::collections::HashMap<String, (usize, usize)> = Default::default();
        for k in 0..n {
            let name = match &g.nodes[k] {
                Node::Obj(o) => o.try_borrow().map(|b| format!("obj:{}", b.tag)).unwrap_or_else(|_| "obj:ocupado".into()),
                Node::Env(_) => "ambiente".into(),
                Node::Clo(_) => "closure".into(),
            };
            let e = hist.entry(name).or_default();
            e.0 += (count[k] > 0) as usize;
            e.1 += live[k] as usize;
        }
        let mut v: Vec<_> = hist.into_iter().collect();
        v.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        for (k, (roots, live)) in v.iter().take(10) {
            eprintln!("[gc] {k}: {live} vivos ({roots} raízes)");
        }
        // formatos (nomes das propriedades) mais comuns entre os objetos vivos
        let mut shapes: std::collections::HashMap<String, usize> = Default::default();
        for k in 0..n {
            if !live[k] {
                continue;
            }
            if let Node::Obj(o) = &g.nodes[k]
                && let Ok(b) = o.try_borrow()
                && b.tag == "Object"
            {
                let keys: Vec<String> = b.props.iter().take(6).map(|(k, _)| match k { crate::value::PropKey::Str(s) => s.to_rust(), _ => "@@sym".into() }).collect();
                *shapes.entry(keys.join(",")).or_default() += 1;
            }
        }
        let mut v: Vec<_> = shapes.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (k, c) in v.iter().take(8) {
            eprintln!("[gc] formato {c}: {{{}}}", k.chars().take(150).collect::<String>());
        }
    }
    // 5. esvazia o lixo (solta as referências; os Rc liberam o ciclo)
    let mut trash: Vec<(Option<ObjRef>, PropMap, Kind)> = Vec::new();
    let mut env_trash: Vec<(Vec<Value>, Option<Env>)> = Vec::new();
    let mut freed = 0;
    for k in 0..n {
        if live[k] {
            continue;
        }
        match &g.nodes[k] {
            Node::Obj(o) => {
                if let Ok(mut b) = o.try_borrow_mut() {
                    freed += 1;
                    let proto = b.proto.take();
                    let props = std::mem::take(&mut b.props);
                    let kind = std::mem::replace(&mut b.kind, Kind::Ordinary);
                    trash.push((proto, props, kind));
                }
            }
            Node::Env(e) => {
                if let Ok(mut b) = e.try_borrow_mut() {
                    freed += 1;
                    let slots = std::mem::take(&mut b.slots);
                    let parent = b.parent.take();
                    env_trash.push((slots, parent));
                }
            }
            Node::Clo(_) => {}
        }
    }
    // as WeakMap vivas varrem as entradas cujas chaves morreram agora
    let weak_live: Vec<ObjRef> = g
        .weak_colls
        .iter()
        .filter(|&&k| live[k as usize])
        .filter_map(|&k| match &g.nodes[k as usize] {
            Node::Obj(o) => Some(ObjRef(o.clone())),
            _ => None,
        })
        .collect();
    drop(g);
    drop(trash);
    drop(env_trash);
    for w in weak_live {
        if let Ok(mut b) = w.0.try_borrow_mut()
            && let Kind::WeakMap(c) | Kind::WeakSet(c) = &mut b.kind
        {
            c.sweep();
        }
    }
    LIVE.with(|c| c.set(n.saturating_sub(freed)));
    SINCE.with(|s| s.set(0));
    Stats { examined: n, freed, ms: t.elapsed().as_secs_f64() * 1000.0 }
}

