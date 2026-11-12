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

