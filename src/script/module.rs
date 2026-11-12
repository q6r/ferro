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

