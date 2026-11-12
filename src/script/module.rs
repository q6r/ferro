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

