//! Módulos ES (`import`/`export`, `import()`, `import.meta`), como no spec:
//! registros de módulo, carga do grafo (LoadRequestedModules), ligação
//! (Link) e avaliação (Evaluate, com `await` no topo).
//!
//! O corpo de um módulo vira uma função com parâmetros que o código não
//! alcança (os nomes começam com `%`):
//!
//! ```text
//! function (%ex, %m0, %m1, ..., %meta) {
//!     %ex("x", () => x, "y", () => y);   // getters das ligações exportadas
//!     <InitialYield>                     // fim do InitializeEnvironment
//!     ...corpo do módulo...
//! }
//! ```
//!
//! `%mN` é o namespace do N-ésimo módulo pedido: um nome importado
//! (`import { a as b } from "x"`) é compilado como `%mN.a` em cada uso, então
//! a ligação fica viva e a TDZ vale (o namespace lança ReferenceError). Na
//! ligação a função é chamada e para no InitialYield, depois de içar as
//! funções; a avaliação retoma o quadro (direto, ou como função async quando
//! o módulo tem `await` no topo).
//!
//! O hospedeiro (página, worker, linha de comando) só resolve especificadores
//! e busca textos ([`ModuleHost`]); o resto mora aqui.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::*;
use crate::builtins::object::Descriptor;
use crate::builtins::promise::AsyncData;
use crate::builtins::{native_data, set_native_data};
use crate::compiler::{self, Code};
use crate::fxhash::{FxHashMap, FxHashSet};
use crate::parser;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Frame, Vm};

type R = Result<Value, Value>;

/// Parâmetro com a função que registra os getters das ligações exportadas.
pub const EXPORTS: &str = "%ex";
/// Parâmetro com o objeto `import.meta`.
pub const META: &str = "%meta";
/// Ligação de `export default <expressão>` e da função/classe sem nome.
pub const DEFAULT_BINDING: &str = "*default*";

fn ns_param(i: usize) -> Name {
    Name::from(format!("%m{i}").as_str())
}

pub type ModuleId = usize;

/// Tipo do módulo, pelo atributo `type` do import.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModType {
    Js,
    Json,
    Text,
    Bytes,
}

impl ModType {
    /// Pelos atributos (Err: não suportados).
    fn from_attrs(attrs: &[(String, String)]) -> Result<ModType, String> {
        let mut ty = ModType::Js;
        for (k, v) in attrs {
            if k != "type" {
                return Err(format!("Import attribute has unsupported key '{k}'"));
            }
            ty = match v.as_str() {
                "json" => ModType::Json,
                "text" => ModType::Text,
                "bytes" => ModType::Bytes,
                _ => return Err(format!("Import attribute has invalid type '{v}'")),
            };
        }
        Ok(ty)
    }
}

/// Conteúdo buscado pelo hospedeiro (Err: mensagem do TypeError).
pub type Fetched = Result<Vec<u8>, String>;

/// O que o hospedeiro oferece ao sistema de módulos.
#[derive(Clone, Copy)]
pub struct ModuleHost {
    /// Resolve um especificador relativo a quem importa (URL de um módulo ou
    /// script; vazio se não há). Err: mensagem do TypeError.
    pub resolve: fn(&mut Vm, &str, &str) -> Result<String, String>,
    /// Começa a buscar o conteúdo de uma URL. Some: já veio (síncrono);
    /// None: o hospedeiro chama [`Vm::module_fetched`] quando chegar.
    pub fetch: fn(&mut Vm, &str) -> Option<Fetched>,
    /// Propriedades do `import.meta` (url, resolve...) do módulo dessa URL.
    pub meta: fn(&mut Vm, &str, &ObjRef),
}

/// Um pedido de módulo (ModuleRequest): especificador, atributos e fase.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub specifier: String,
    pub attrs: ImportAttrs,
    pub phase: ImportPhase,
}

/// O que uma importação (ou reexportação) pega do outro módulo.
#[derive(Clone, Debug, PartialEq)]
enum What {
    Name(JsStr),
    /// `* as ns`
    Namespace,
    /// `import source x`
    Source,
}

struct ImportEntry {
    req: usize,
    what: What,
}

/// `export { a as b } from "x"`, `export * as ns from "x"` e reexportação
/// de um nome importado.
struct IndirectExport {
    name: JsStr,
    req: usize,
    what: What,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    New,
    Unlinked,
    Linking,
    Linked,
    Evaluating,
    EvaluatingAsync,
    Evaluated,
}

/// Resultado do ResolveExport.
#[derive(Clone, PartialEq)]
enum Resolved {
    Binding(ModuleId, JsStr),
    Namespace(ModuleId),
    Source(ModuleId),
    /// Não existe.
    Null,
    /// Ciclo de reexportações.
    Circular,
    /// Duas `export *` dão ligações diferentes.
    Ambiguous,
}

/// Um registro de módulo (Source Text Module Record, ou sintético: JSON,
/// texto, bytes).
pub struct Module {
    pub url: Rc<str>,
    pub ty: ModType,
    pub requests: Vec<Request>,
    /// Módulo carregado para cada pedido ([[LoadedModules]]).
    loaded: Vec<Option<ModuleId>>,
    imports: Vec<ImportEntry>,
    local_exports: Vec<(JsStr, JsStr)>,
    indirect_exports: Vec<IndirectExport>,
    star_exports: Vec<usize>,
    pub has_tla: bool,
    code: Option<Rc<Code>>,
    /// Módulo sintético: o valor do `default`.
    synthetic: Option<Value>,
    pub status: Status,
    pub eval_error: Option<Value>,
    dfs_index: usize,
    dfs_anc: usize,
    cycle_root: ModuleId,
    /// [[AsyncEvaluation]]: a ordem em que virou true (0 = nunca).
    async_order: u64,
    async_evaluating: bool,
    /// [[TopLevelCapability]]
    tlc: Option<ObjRef>,
    async_parents: Vec<ModuleId>,
    pending_deps: usize,
    /// O quadro da função do módulo, parado no InitialYield (o ambiente).
    frame: Option<Box<Frame>>,
    /// Getters das ligações locais exportadas (nome local → função).
    getters: FxHashMap<JsStr, Value>,
    /// Muda quando a ligação falha e o ambiente é descartado (os getters
    /// guardados nos namespaces deixam de valer).
    generation: u32,
    namespace: Option<ObjRef>,
    deferred_ns: Option<ObjRef>,
}

/// Erro ao carregar o grafo: o valor e se foi falha de busca (rede, arquivo).
#[derive(Clone)]
pub struct LoadErr {
    pub value: Value,
    pub fetch: bool,
}

/// Continuação do hospedeiro quando um grafo termina de carregar.
type GraphCont = Box<dyn FnOnce(&mut Vm, Result<(), LoadErr>)>;

/// O que fazer quando um grafo termina de carregar.
enum GraphDone {
    /// LoadRequestedModules do `import()`: a promessa.
    Promise(ObjRef),
    Host(GraphCont),
}

/// GraphLoadingState
struct GraphLoad {
    loading: bool,
    pending: usize,
    visited: Vec<ModuleId>,
    done: Option<GraphDone>,
}

type Graph = Rc<RefCell<GraphLoad>>;

/// Quem espera um módulo carregar (o `payload` do HostLoadImportedModule).
#[derive(Clone)]
enum Payload {
    Graph(Graph),
    /// `import()`: a promessa e a fase.
    Dynamic(ObjRef, ImportPhase),
    /// O módulo principal de um hospedeiro (`run_module`).
    Root(Rc<RefCell<Option<RootDone>>>),
}

type RootDone = Box<dyn FnOnce(&mut Vm, Ran)>;

struct Pending {
    referrer: Option<(ModuleId, usize)>,
    payload: Payload,
}

enum Entry {
    Fetching(Vec<Pending>),
    Done(Result<ModuleId, LoadErr>),
}

/// Como terminou um módulo principal (`run_module`).
pub enum Ran {
    /// Algum módulo do grafo não pôde ser buscado.
    FetchFailed(Value),
    /// Erro de sintaxe ou de ligação.
    Failed(Value),
    /// A avaliação começou: a promessa dela (rejeita com o erro do corpo).
    Started(Value),
}

/// Os módulos de uma realm.
#[derive(Default)]
pub struct Modules {
    pub host: Option<ModuleHost>,
    pub list: Vec<Module>,
    map: HashMap<(Rc<str>, ModType), Entry>,
    order: u64,
}

impl Modules {
    /// O módulo dessa URL (já carregado).
    pub fn find(&self, url: &str) -> Option<ModuleId> {
        self.map.iter().find_map(|((u, _), s)| match s {
            Entry::Done(Ok(id)) if &**u == url => Some(*id),
            _ => None,
        })
    }

    fn is_source(&self, id: ModuleId) -> bool {
        self.list[id].synthetic.is_none()
    }
}

// ------------------------------------------------------------ compilação

/// Nomes que uma declaração exportada cria.
fn declared_names(s: &Stmt, out: &mut Vec<Name>) {
    match s {
        Stmt::Var(_, decls, _) => {
            for d in decls {
                compiler::bound_names(&d.target, out);
            }
        }
        Stmt::Function(f) => out.extend(f.name.clone()),
        Stmt::Class(c) => out.extend(c.name.clone()),
        _ => {}
    }
}

fn syntax_err(vm: &mut Vm, msg: &str, url: &str, line: u32) -> Value {
    vm.syntax_error(format!("{msg} ({url}:{line})"))
}

fn ident(n: &str) -> Expr {
    Expr::Ident(Name::from(n), 0)
}

/// Há `return` fora de funções (no topo do módulo)?
fn returns(ss: &[Stmt]) -> bool {
    ss.iter().any(|s| match s {
        Stmt::Return(..) => true,
        Stmt::If(_, a, b) => returns(std::slice::from_ref(a)) || b.as_ref().is_some_and(|b| returns(std::slice::from_ref(b))),
        Stmt::Block(b) => returns(b),
        Stmt::For { body, .. } | Stmt::ForIn { body, .. } | Stmt::ForOf { body, .. } | Stmt::While(_, body) | Stmt::DoWhile(body, _) => {
            returns(std::slice::from_ref(body))
        }
        Stmt::Labeled(_, b) | Stmt::With(_, b) => returns(std::slice::from_ref(b)),
        Stmt::Try { block, handler, finalizer, .. } => {
            returns(block) || handler.as_deref().is_some_and(returns) || finalizer.as_deref().is_some_and(returns)
        }
        Stmt::Switch(_, cases) => cases.iter().any(|c| returns(&c.body)),
        _ => false,
    })
}

/// `() => nome` (getter de uma ligação exportada).
fn getter(n: &Name) -> Expr {
    Expr::Function(Rc::new(FunctionDef {
        name: None,
        params: Vec::new(),
        rest: None,
        body: FnBody::Expr(Box::new(Expr::Ident(n.clone(), 0))),
        kind: FnKind::Arrow,
        is_async: false,
        is_generator: false,
        strict: true,
        source: Rc::from(""),
        line: 0,
    }))
}

/// ParseModule: analisa o texto, confere os erros antecipados de módulo e
/// compila a função do módulo.
fn parse_module(vm: &mut Vm, src: &str, url: &str) -> Result<Module, Value> {
    let items = parser::parse_module(src).map_err(|e| syntax_err(vm, &e.msg, url, e.line))?;
    let mut requests: Vec<Request> = Vec::new();
    let mut request = |spec: &str, attrs: &ImportAttrs, phase: ImportPhase| -> usize {
        let mut attrs = attrs.clone();
        attrs.sort();
        let r = Request { specifier: spec.to_string(), attrs, phase };
        match requests.iter().position(|x| *x == r) {
            Some(i) => i,
            None => {
                requests.push(r);
                requests.len() - 1
            }
        }
    };
    let mut imports: Vec<ImportEntry> = Vec::new();
    // nome local → (parâmetro do namespace, nome exportado lá)
    let mut import_map: FxHashMap<Name, (Name, Option<Name>)> = FxHashMap::default();
    let mut import_locals: Vec<(Name, usize)> = Vec::new();
    let mut local_exports: Vec<(JsStr, Name)> = Vec::new();
    let mut indirect: Vec<IndirectExport> = Vec::new();
    let mut star: Vec<usize> = Vec::new();
    // `export { x }`: x precisa existir no módulo
    let mut referenced: Vec<Name> = Vec::new();
    let mut body: Vec<Stmt> = Vec::new();
    for item in items {
        match item {
            ModuleItem::Stmt(s) => body.push(s),
            ModuleItem::Import(d) => {
                let req = request(&d.specifier, &d.attrs, d.phase);
                for (what, local) in d.bindings {
                    let what = match (what, d.phase) {
                        (_, ImportPhase::Source) => What::Source,
                        (ImportName::Namespace, _) => What::Namespace,
                        (ImportName::Default, _) => What::Name(JsStr::new("default")),
                        (ImportName::Named(n), _) => What::Name(JsStr::new(&n)),
                    };
                    let export = match &what {
                        What::Name(n) => Some(Name::from(n.to_rust().as_str())),
                        _ => None,
                    };
                    import_map.insert(local.clone(), (ns_param(req), export));
                    import_locals.push((local, imports.len()));
                    imports.push(ImportEntry { req, what });
                }
            }
            ModuleItem::Export(e) => match e {
                ExportDecl::Decl(s) => {
                    let mut names = Vec::new();
                    declared_names(&s, &mut names);
                    for n in names {
                        local_exports.push((JsStr::new(&n), n));
                    }
                    body.push(s);
                }
                ExportDecl::DefaultDecl(s, n) => {
                    let s = match s {
                        // `export default function () {}`: declaração içada
                        Stmt::Function(mut f) if f.name.is_none() => {
                            match Rc::get_mut(&mut f) {
                                Some(def) => def.name = Some(n.clone()),
                                None => return Err(vm.syntax_error("módulo: função compartilhada")),
                            }
                            Stmt::Function(f)
                        }
                        s => s,
                    };
                    local_exports.push((JsStr::new("default"), n));
                    body.push(s);
                }
                ExportDecl::DefaultExpr(e) => {
                    local_exports.push((JsStr::new("default"), Name::from(DEFAULT_BINDING)));
                    body.push(Stmt::Var(
                        VarKind::Const,
                        vec![VarDecl {
                            target: Pattern::Ident(Name::from(DEFAULT_BINDING)),
                            init: Some(e),
                        }],
                        0,
                    ));
                }
                ExportDecl::Names(list) => {
                    for (local, exported) in list {
                        referenced.push(local.clone());
                        local_exports.push((JsStr::new(&exported), local));
                    }
                }
                ExportDecl::From(spec, list, attrs) => {
                    let req = request(&spec, &attrs, ImportPhase::Evaluation);
                    for (theirs, exported) in list {
                        indirect.push(IndirectExport {
                            name: JsStr::new(&exported),
                            req,
                            what: What::Name(JsStr::new(&theirs)),
                        });
                    }
                }
                ExportDecl::All(spec, alias, attrs) => {
                    let req = request(&spec, &attrs, ImportPhase::Evaluation);
                    match alias {
                        Some(a) => indirect.push(IndirectExport {
                            name: JsStr::new(&a),
                            req,
                            what: What::Namespace,
                        }),
                        None => star.push(req),
                    }
                }
            },
        }
    }
    // erros antecipados: no topo de um módulo, funções, let/const/class e
    // imports são léxicos (sem repetição, sem `var` com o mesmo nome)
    if returns(&body) {
        return Err(syntax_err(vm, "Illegal return statement", url, 0));
    }
    let mut vars = Vec::new();
    compiler::var_names(&body, &mut vars, false, false);
    let mut lex = Vec::new();
    compiler::lexical_decls(&body, true, &mut lex);
    let mut top: FxHashSet<Name> = FxHashSet::default();
    let lexical = import_locals.iter().map(|(n, _)| n).chain(lex.iter().map(|(n, _)| n));
    for n in lexical {
        if !top.insert(n.clone()) || vars.contains(n) {
            return Err(syntax_err(vm, &format!("Identifier '{n}' has already been declared"), url, 0));
        }
    }
    top.extend(vars);
    for n in &referenced {
        if !top.contains(n) {
            return Err(syntax_err(vm, &format!("Export '{n}' is not defined in module"), url, 0));
        }
    }
    {
        let mut seen: FxHashSet<&[u16]> = FxHashSet::default();
        let all = local_exports.iter().map(|(e, _)| e).chain(indirect.iter().map(|e| &e.name));
        for e in all {
            if !seen.insert(e.units()) {
                return Err(syntax_err(vm, &format!("Duplicate export of '{e}'"), url, 0));
            }
        }
    }
    // reexportação de um nome importado vira indireta (a não ser `* as`)
    let mut locals: Vec<(JsStr, JsStr)> = Vec::new();
    let mut exported_locals: Vec<Name> = Vec::new();
    for (exported, local) in local_exports {
        if let Some((_, i)) = import_locals.iter().find(|(n, _)| *n == local) {
            let ie = &imports[*i];
            // `import * as ns; export { ns }` também vira indireta (ResolveExport
            // dá o namespace); só o namespace adiado continua local
            if requests[ie.req].phase != ImportPhase::Defer {
                indirect.push(IndirectExport {
                    name: exported,
                    req: ie.req,
                    what: ie.what.clone(),
                });
                continue;
            }
        }
        if !exported_locals.contains(&local) {
            exported_locals.push(local.clone());
        }
        locals.push((exported, JsStr::new(&local)));
    }
    let has_tla = compiler::has_top_level_await(&body);
    // a função do módulo: %ex("x", () => x, ...); corpo
    let mut stmts = Vec::with_capacity(body.len() + 1);
    if !exported_locals.is_empty() {
        let mut args = Vec::new();
        for n in &exported_locals {
            let units: Vec<u16> = n.encode_utf16().collect();
            args.push(ArrayElem::Expr(Expr::Str(Rc::from(units))));
            args.push(ArrayElem::Expr(getter(n)));
        }
        stmts.push(Stmt::Expr(
            Expr::Call {
                callee: Box::new(ident(EXPORTS)),
                args,
                optional: false,
                line: 0,
            },
            0,
        ));
    }
    stmts.extend(body);
    let mut params = vec![Pattern::Ident(Name::from(EXPORTS))];
    for i in 0..requests.len() {
        params.push(Pattern::Ident(ns_param(i)));
    }
    params.push(Pattern::Ident(Name::from(META)));
    let def = FunctionDef {
        name: None,
        params,
        rest: None,
        body: FnBody::Block(stmts),
        kind: FnKind::Normal,
        is_async: true,
        is_generator: false,
        strict: true,
        source: Rc::from(""),
        line: 1,
    };
    let prog = Program {
        body: vec![Stmt::Expr(Expr::Function(Rc::new(def)), 1)],
        strict: true,
        imports: import_map,
    };
    let code = compiler::compile_module(&prog, url).map_err(|e| syntax_err(vm, &e.msg, url, e.line))?;
    let Some(code) = code.functions.first().cloned() else {
        return Err(vm.syntax_error("módulo: função ausente"));
    };
    let n = requests.len();
    Ok(Module {
        url: Rc::from(url),
        ty: ModType::Js,
        requests,
        loaded: vec![None; n],
        imports,
        local_exports: locals,
        indirect_exports: indirect,
        star_exports: star,
        has_tla,
        code: Some(code),
        synthetic: None,
        status: Status::New,
        eval_error: None,
        dfs_index: 0,
        dfs_anc: 0,
        cycle_root: 0,
        async_order: 0,
        async_evaluating: false,
        tlc: None,
        async_parents: Vec::new(),
        pending_deps: 0,
        frame: None,
        getters: FxHashMap::default(),
        generation: 0,
        namespace: None,
        deferred_ns: None,
    })
}

/// Módulo sintético com um único `export default`.
fn synthetic(url: &str, ty: ModType, value: Value) -> Module {
    Module {
        url: Rc::from(url),
        ty,
        requests: Vec::new(),
        loaded: Vec::new(),
        imports: Vec::new(),
        local_exports: Vec::new(),
        indirect_exports: Vec::new(),
        star_exports: Vec::new(),
        has_tla: false,
        code: None,
        synthetic: Some(value),
        status: Status::Unlinked,
        eval_error: None,
        dfs_index: 0,
        dfs_anc: 0,
        cycle_root: 0,
        async_order: 0,
        async_evaluating: false,
        tlc: None,
        async_parents: Vec::new(),
        pending_deps: 0,
        frame: None,
        getters: FxHashMap::default(),
        generation: 0,
        namespace: None,
        deferred_ns: None,
    }
}

/// Texto de um módulo (UTF-8, sem BOM).
fn decode(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8_lossy(b).into_owned()
}

/// Cria o registro do módulo a partir do conteúdo buscado.
fn create_module(vm: &mut Vm, url: &str, ty: ModType, bytes: Vec<u8>) -> Result<ModuleId, Value> {
    let mut m = match ty {
        ModType::Js => parse_module(vm, &decode(&bytes), url)?,
        ModType::Json => {
            let text = decode(&bytes);
            let v = crate::builtins::json::parse_json(vm, &text)?;
            synthetic(url, ty, v)
        }
        ModType::Text => synthetic(url, ty, Value::String(JsStr::new(&decode(&bytes)))),
        ModType::Bytes => {
            let v = crate::builtins::typed::immutable_bytes(vm, bytes);
            synthetic(url, ty, v)
        }
    };
    let id = vm.modules.list.len();
    m.cycle_root = id;
    vm.modules.list.push(m);
    Ok(id)
}

// ------------------------------------------------------------ carga

/// HostLoadImportedModule: resolve o pedido e entrega o módulo a `payload`
/// (agora, se já está carregado ou a busca é síncrona; ou quando chegar).
fn host_load(vm: &mut Vm, referrer: Option<(ModuleId, usize)>, referrer_url: &str, req: &Request, payload: Payload) {
    let ty = match ModType::from_attrs(&req.attrs) {
        Ok(t) => t,
        Err(msg) => {
            let e = vm.syntax_error(msg);
            return finish_loading(vm, referrer, payload, Err(LoadErr { value: e, fetch: false }));
        }
    };
    let Some(host) = vm.modules.host else {
        let e = vm.type_error("Cannot load modules here");
        return finish_loading(vm, referrer, payload, Err(LoadErr { value: e, fetch: true }));
    };
    let url = match (host.resolve)(vm, &req.specifier, referrer_url) {
        Ok(u) => u,
        Err(msg) => {
            let e = vm.type_error(msg);
            return finish_loading(vm, referrer, payload, Err(LoadErr { value: e, fetch: false }));
        }
    };
    load_url(vm, &url, ty, referrer, payload);
}

/// Pede o módulo `url` (do tipo `ty`) para `payload`.
fn load_url(vm: &mut Vm, url: &str, ty: ModType, referrer: Option<(ModuleId, usize)>, payload: Payload) {
    let key: (Rc<str>, ModType) = (Rc::from(url), ty);
    match vm.modules.map.get_mut(&key) {
        Some(Entry::Done(r)) => {
            let r = r.clone();
            finish_loading(vm, referrer, payload, r);
        }
        Some(Entry::Fetching(list)) => list.push(Pending { referrer, payload }),
        None => {
            vm.modules.map.insert(key, Entry::Fetching(vec![Pending { referrer, payload }]));
            let Some(host) = vm.modules.host else { return };
            if let Some(r) = (host.fetch)(vm, url) {
                vm.module_fetched(url, r);
            }
        }
    }
}

/// FinishLoadingImportedModule
fn finish_loading(vm: &mut Vm, referrer: Option<(ModuleId, usize)>, payload: Payload, result: Result<ModuleId, LoadErr>) {
    if let (Ok(m), Some((r, i))) = (&result, referrer)
        && let Some(slot) = vm.modules.list[r].loaded.get_mut(i)
        && slot.is_none()
    {
        *slot = Some(*m);
    }
    match payload {
        Payload::Graph(g) => continue_loading(vm, &g, result),
        Payload::Dynamic(p, phase) => continue_dynamic_import(vm, p, phase, result.map_err(|e| e.value)),
        Payload::Root(done) => {
            let Some(done) = done.borrow_mut().take() else { return };
            match result {
                Ok(id) => run_loaded(vm, id, done),
                Err(e) if e.fetch => done(vm, Ran::FetchFailed(e.value)),
                Err(e) => done(vm, Ran::Failed(e.value)),
            }
        }
    }
}

/// LoadRequestedModules
fn load_requested(vm: &mut Vm, id: ModuleId, done: GraphDone) {
    let g = Rc::new(RefCell::new(GraphLoad {
        loading: true,
        pending: 1,
        visited: Vec::new(),
        done: Some(done),
    }));
    inner_load(vm, &g, id);
}

/// InnerModuleLoading
fn inner_load(vm: &mut Vm, g: &Graph, id: ModuleId) {
    let fresh = vm.modules.is_source(id) && vm.modules.list[id].status == Status::New && !g.borrow().visited.contains(&id);
    if fresh {
        let n = vm.modules.list[id].requests.len();
        {
            let mut b = g.borrow_mut();
            b.visited.push(id);
            b.pending += n;
        }
        let url = vm.modules.list[id].url.clone();
        for i in 0..n {
            let req = vm.modules.list[id].requests[i].clone();
            if let Err(msg) = ModType::from_attrs(&req.attrs) {
                let e = vm.syntax_error(msg);
                continue_loading(vm, g, Err(LoadErr { value: e, fetch: false }));
            } else if let Some(m) = vm.modules.list[id].loaded[i] {
                inner_load(vm, g, m);
            } else {
                host_load(vm, Some((id, i)), &url, &req, Payload::Graph(g.clone()));
            }
            if !g.borrow().loading {
                return;
            }
        }
    }
    let finished = {
        let mut b = g.borrow_mut();
        b.pending -= 1;
        if b.pending == 0 {
            b.loading = false;
            Some((std::mem::take(&mut b.visited), b.done.take()))
        } else {
            None
        }
    };
    if let Some((visited, done)) = finished {
        for m in visited {
            if vm.modules.list[m].status == Status::New {
                vm.modules.list[m].status = Status::Unlinked;
            }
        }
        match done {
            Some(GraphDone::Promise(p)) => vm.resolve_promise(&p, Value::Undefined),
            Some(GraphDone::Host(f)) => f(vm, Ok(())),
            None => {}
        }
    }
}

/// ContinueModuleLoading
fn continue_loading(vm: &mut Vm, g: &Graph, result: Result<ModuleId, LoadErr>) {
    if !g.borrow().loading {
        return;
    }
    match result {
        Ok(m) => inner_load(vm, g, m),
        Err(e) => {
            let done = {
                let mut b = g.borrow_mut();
                b.loading = false;
                b.done.take()
            };
            match done {
                Some(GraphDone::Promise(p)) => vm.reject_promise(&p, e.value),
                Some(GraphDone::Host(f)) => f(vm, Err(e)),
                None => {}
            }
        }
    }
}

/// O módulo principal carregou: carrega o grafo, liga e avalia.
fn run_loaded(vm: &mut Vm, id: ModuleId, done: RootDone) {
    load_requested(
        vm,
        id,
        GraphDone::Host(Box::new(move |vm, r| match r {
            Err(e) if e.fetch => done(vm, Ran::FetchFailed(e.value)),
            Err(e) => done(vm, Ran::Failed(e.value)),
            Ok(()) => match link(vm, id) {
                Err(e) => done(vm, Ran::Failed(e)),
                Ok(()) => {
                    let p = evaluate(vm, id);
                    done(vm, Ran::Started(Value::Object(p)))
                }
            },
        })),
    );
}

impl Vm {
    /// Roda o módulo `url` com as dependências (carrega, liga e avalia).
    /// `text`: o texto já veio (módulo inline); None: o hospedeiro busca.
    /// `done` é chamada quando a avaliação começa (ou algo falha).
    pub fn run_module(&mut self, url: &str, text: Option<String>, done: RootDone) {
        let payload = Payload::Root(Rc::new(RefCell::new(Some(done))));
        match text {
            None => load_url(self, url, ModType::Js, None, payload),
            Some(t) => {
                let r = create_module(self, url, ModType::Js, t.into_bytes()).map_err(|e| LoadErr { value: e, fetch: false });
                self.modules.map.insert((Rc::from(url), ModType::Js), Entry::Done(r.clone()));
                finish_loading(self, None, payload, r);
            }
        }
    }

    /// Começa a buscar o módulo `url` e as dependências (sem rodar).
    pub fn prefetch_module(&mut self, url: &str) {
        let done: RootDone = Box::new(|_, _| {});
        let payload = Payload::Graph(Rc::new(RefCell::new(GraphLoad {
            loading: true,
            pending: 1,
            visited: Vec::new(),
            done: Some(GraphDone::Host(Box::new(move |vm, _| done(vm, Ran::Started(Value::Undefined))))),
        })));
        load_url(self, url, ModType::Js, None, payload);
    }

    /// O conteúdo de `url` chegou (ou a busca falhou): entrega a quem espera.
    pub fn module_fetched(&mut self, url: &str, result: Fetched) {
        let keys: Vec<(Rc<str>, ModType)> = self
            .modules
            .map
            .iter()
            .filter(|((u, _), s)| &**u == url && matches!(s, Entry::Fetching(_)))
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            let r = match &result {
                Ok(bytes) => create_module(self, url, key.1, bytes.clone()).map_err(|e| LoadErr { value: e, fetch: false }),
                Err(msg) => {
                    let e = self.type_error(msg.clone());
                    Err(LoadErr { value: e, fetch: true })
                }
            };
            let Some(Entry::Fetching(waiters)) = self.modules.map.insert(key, Entry::Done(r.clone())) else { continue };
            for w in waiters {
                finish_loading(self, w.referrer, w.payload, r.clone());
            }
        }
    }
}

// ------------------------------------------------------------ resolução

/// GetExportedNames
fn exported_names(mods: &Modules, id: ModuleId, set: &mut Vec<ModuleId>, out: &mut Vec<JsStr>) {
    let m = &mods.list[id];
    if m.synthetic.is_some() {
        out.push(JsStr::new("default"));
        return;
    }
    if set.contains(&id) {
        return;
    }
    set.push(id);
    for (e, _) in &m.local_exports {
        out.push(e.clone());
    }
    for e in &m.indirect_exports {
        out.push(e.name.clone());
    }
    for &req in &m.star_exports {
        let Some(target) = m.loaded[req] else { continue };
        let mut names = Vec::new();
        exported_names(mods, target, set, &mut names);
        for n in names {
            if !n.eq_str("default") && !out.contains(&n) {
                out.push(n);
            }
        }
    }
}

/// ResolveExport
fn resolve_export(mods: &Modules, id: ModuleId, name: &JsStr, set: &mut Vec<(ModuleId, JsStr)>) -> Resolved {
    let m = &mods.list[id];
    if m.synthetic.is_some() {
        return if name.eq_str("default") { Resolved::Binding(id, name.clone()) } else { Resolved::Null };
    }
    if set.iter().any(|(x, n)| *x == id && n == name) {
        return Resolved::Circular;
    }
    set.push((id, name.clone()));
    for (e, l) in &m.local_exports {
        if e == name {
            return Resolved::Binding(id, l.clone());
        }
    }
    for e in &m.indirect_exports {
        if e.name == *name {
            let Some(target) = m.loaded[e.req] else { return Resolved::Null };
            return match &e.what {
                What::Namespace => Resolved::Namespace(target),
                What::Source => Resolved::Source(target),
                What::Name(n) => resolve_export(mods, target, n, set),
            };
        }
    }
    if name.eq_str("default") {
        return Resolved::Null;
    }
    let mut star: Option<Resolved> = None;
    for &req in &m.star_exports {
        let Some(target) = m.loaded[req] else { continue };
        match resolve_export(mods, target, name, set) {
            Resolved::Ambiguous => return Resolved::Ambiguous,
            Resolved::Null | Resolved::Circular => {}
            r => match &star {
                None => star = Some(r),
                Some(s) if *s != r => return Resolved::Ambiguous,
                Some(_) => {}
            },
        }
    }
    star.unwrap_or(Resolved::Null)
}

/// SyntaxError de ligação para um nome que não resolveu.
fn link_error(vm: &mut Vm, r: &Resolved, name: &JsStr, spec: &str) -> Value {
    match r {
        Resolved::Ambiguous => vm.syntax_error(format!("The requested module '{spec}' contains conflicting star exports for name '{name}'")),
        Resolved::Circular => vm.syntax_error(format!("Detected cycle while resolving name '{name}' in '{spec}'")),
        _ => vm.syntax_error(format!("The requested module '{spec}' does not provide an export named '{name}'")),
    }
}

// ------------------------------------------------------------ namespace

/// Para onde um nome do namespace aponta.
enum Target {
    Binding(ModuleId, JsStr),
    Namespace(ModuleId),
    Source(ModuleId),
}

/// Dados do namespace de um módulo (o handler interno do Proxy).
pub struct NsData {
    pub module: ModuleId,
    /// Namespace de `import defer`: avaliar o módulo no primeiro acesso.
    pub deferred: bool,
    /// Nomes exportados, em ordem de código UTF-16.
    names: Vec<JsStr>,
    index: FxHashMap<JsStr, usize>,
    targets: Vec<Target>,
    /// Getter de cada ligação já achado (com a geração do módulo dono).
    cache: RefCell<Vec<Option<(u32, Value)>>>,
}

/// GetModuleNamespace (ou o namespace adiado de `import defer`).
pub fn namespace(vm: &mut Vm, id: ModuleId, deferred: bool) -> ObjRef {
    let cached = if deferred { &vm.modules.list[id].deferred_ns } else { &vm.modules.list[id].namespace };
    if let Some(ns) = cached {
        return ns.clone();
    }
    let mut names = Vec::new();
    exported_names(&vm.modules, id, &mut Vec::new(), &mut names);
    let mut entries: Vec<(JsStr, Target)> = Vec::new();
    for n in names {
        match resolve_export(&vm.modules, id, &n, &mut Vec::new()) {
            Resolved::Binding(m, b) => entries.push((n, Target::Binding(m, b))),
            Resolved::Namespace(m) => entries.push((n, Target::Namespace(m))),
            Resolved::Source(m) => entries.push((n, Target::Source(m))),
            _ => {}
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut data = NsData {
        module: id,
        deferred,
        names: Vec::with_capacity(entries.len()),
        index: FxHashMap::default(),
        targets: Vec::with_capacity(entries.len()),
        cache: RefCell::new(vec![None; entries.len()]),
    };
    for (i, (n, t)) in entries.into_iter().enumerate() {
        data.index.insert(n.clone(), i);
        data.names.push(n);
        data.targets.push(t);
    }
    // alvo: guarda o @@toStringTag (as chaves símbolo são comuns)
    let target = ObjRef::new(Object::new(None, Kind::Ordinary));
    {
        let mut b = target.borrow_mut();
        let tag = if deferred { "Deferred Module" } else { "Module" };
        b.props.insert(
            PropKey::Sym(vm.realm.sym_to_string_tag.clone()),
            Property {
                slot: Slot::Data { value: Value::str(tag), writable: false },
                enumerable: false,
                configurable: false,
            },
        );
        b.extensible = false;
    }
    let handler = ObjRef::new(Object::new(None, Kind::ModuleNs(Rc::new(data))));
    let mut obj = Object::new(None, Kind::Proxy { target: Some(target), handler: Some(handler) });
    obj.tag = "Module";
    obj.extensible = false;
    let ns = ObjRef::new(obj);
    let m = &mut vm.modules.list[id];
    if deferred {
        m.deferred_ns = Some(ns.clone());
    } else {
        m.namespace = Some(ns.clone());
    }
    ns
}

/// Os dados do namespace, se `o` for um.
pub fn namespace_data(o: &ObjRef) -> Option<Rc<NsData>> {
    let b = o.borrow();
    let Kind::Proxy { handler: Some(h), .. } = &b.kind else { return None };
    let hb = h.borrow();
    match &hb.kind {
        Kind::ModuleNs(d) => Some(d.clone()),
        _ => None,
    }
}

fn ns_target(o: &ObjRef) -> ObjRef {
    match &o.borrow().kind {
        Kind::Proxy { target: Some(t), .. } => t.clone(),
        _ => o.clone(),
    }
}

/// Valor atual da ligação `i` do namespace.
fn ns_value(vm: &mut Vm, ns: &NsData, i: usize) -> R {
    match &ns.targets[i] {
        Target::Namespace(m) => Ok(Value::Object(namespace(vm, *m, false))),
        Target::Source(m) => module_source(vm, *m),
        Target::Binding(m, local) => {
            let module = &vm.modules.list[*m];
            let generation = module.generation;
            let cached = match &ns.cache.borrow()[i] {
                Some((g, f)) if *g == generation => Some(f.clone()),
                _ => None,
            };
            if let Some(f) = cached {
                return vm.call(&f, Value::Undefined, Vec::new());
            }
            if let Some(v) = &module.synthetic {
                return Ok(v.clone());
            }
            match module.getters.get(local).cloned() {
                Some(f) => {
                    ns.cache.borrow_mut()[i] = Some((generation, f.clone()));
                    vm.call(&f, Value::Undefined, Vec::new())
                }
                None => Err(vm.reference_error(format!("{local} is not defined"))),
            }
        }
    }
}

/// IsSymbolLikeNamespaceKey: símbolos (e `then` num namespace adiado) são
/// propriedades comuns do alvo; o resto é nome exportado (e num namespace
/// adiado avalia o módulo antes).
fn ns_key<'a>(vm: &mut Vm, ns: &NsData, key: &'a PropKey) -> Result<Option<&'a JsStr>, Value> {
    match key {
        PropKey::Str(s) if ns.deferred && s.eq_str("then") => Ok(None),
        PropKey::Str(s) => {
            if ns.deferred {
                ensure_deferred_evaluated(vm, ns.module)?;
            }
            Ok(Some(s))
        }
        PropKey::Sym(_) => Ok(None),
    }
}

/// As operações do namespace (objeto exótico), chamadas pelo Proxy.
impl Vm {
    pub fn ns_get(&mut self, o: &ObjRef, ns: &NsData, key: &PropKey, receiver: &Value) -> R {
        let Some(s) = ns_key(self, ns, key)? else {
            return self.get_with(&ns_target(o), key, receiver);
        };
        match ns.index.get(s) {
            Some(&i) => ns_value(self, ns, i),
            None => Ok(Value::Undefined),
        }
    }

    pub fn ns_has(&mut self, o: &ObjRef, ns: &NsData, key: &PropKey) -> Result<bool, Value> {
        let Some(s) = ns_key(self, ns, key)? else {
            return self.has_property(&ns_target(o), key);
        };
        Ok(ns.index.contains_key(s))
    }

    pub fn ns_delete(&mut self, o: &ObjRef, ns: &NsData, key: &PropKey) -> Result<bool, Value> {
        let Some(s) = ns_key(self, ns, key)? else {
            return Ok(self.delete_prop(&ns_target(o), key));
        };
        Ok(!ns.index.contains_key(s))
    }

    pub fn ns_own_keys(&mut self, o: &ObjRef, ns: &NsData) -> Result<Vec<PropKey>, Value> {
        if ns.deferred {
            ensure_deferred_evaluated(self, ns.module)?;
        }
        let mut keys: Vec<PropKey> = ns.names.iter().map(|n| PropKey::Str(n.clone())).collect();
        keys.extend(self.own_keys(&ns_target(o))?);
        Ok(keys)
    }

    pub fn ns_get_own_property(&mut self, o: &ObjRef, ns: &NsData, key: &PropKey) -> Result<Option<Property>, Value> {
        let Some(s) = ns_key(self, ns, key)? else {
            return Ok(ns_target(o).borrow().own(key));
        };
        let Some(&i) = ns.index.get(s) else { return Ok(None) };
        let value = ns_value(self, ns, i)?;
        Ok(Some(Property {
            slot: Slot::Data { value, writable: true },
            enumerable: true,
            configurable: false,
        }))
    }

    pub fn ns_define(&mut self, o: &ObjRef, ns: &NsData, key: PropKey, d: Descriptor) -> Result<bool, Value> {
        if ns_key(self, ns, &key)?.is_none() {
            return crate::builtins::object::define_own_property(self, &ns_target(o), key, d);
        }
        let Some(cur) = self.ns_get_own_property(o, ns, &key)? else { return Ok(false) };
        if d.configurable == Some(true) || d.enumerable == Some(false) || d.is_accessor() || d.writable == Some(false) {
            return Ok(false);
        }
        match (d.value, cur.slot) {
            (Some(v), Slot::Data { value, .. }) => Ok(crate::vm::same_value(&v, &value)),
            _ => Ok(true),
        }
    }
}

// ------------------------------------------------------------ ligação

/// Link()
pub fn link(vm: &mut Vm, id: ModuleId) -> Result<(), Value> {
    let mut stack = Vec::new();
    if let Err(e) = inner_link(vm, id, &mut stack, 0) {
        for m in stack {
            let m = &mut vm.modules.list[m];
            m.status = Status::Unlinked;
            m.frame = None;
            m.getters.clear();
            m.generation += 1;
        }
        return Err(e);
    }
    Ok(())
}

/// InnerModuleLinking
fn inner_link(vm: &mut Vm, id: ModuleId, stack: &mut Vec<ModuleId>, mut index: usize) -> Result<usize, Value> {
    if !vm.modules.is_source(id) {
        if vm.modules.list[id].status == Status::Unlinked {
            vm.modules.list[id].status = Status::Linked;
        }
        return Ok(index);
    }
    match vm.modules.list[id].status {
        Status::Linking | Status::Linked | Status::Evaluating | Status::EvaluatingAsync | Status::Evaluated => return Ok(index),
        _ => {}
    }
    {
        let m = &mut vm.modules.list[id];
        m.status = Status::Linking;
        m.dfs_index = index;
        m.dfs_anc = index;
    }
    index += 1;
    stack.push(id);
    for i in 0..vm.modules.list[id].requests.len() {
        let Some(req) = vm.modules.list[id].loaded[i] else {
            let spec = vm.modules.list[id].requests[i].specifier.clone();
            return Err(vm.type_error(format!("Failed to load module '{spec}'")));
        };
        index = inner_link(vm, req, stack, index)?;
        if vm.modules.is_source(req) && vm.modules.list[req].status == Status::Linking {
            let anc = vm.modules.list[req].dfs_anc;
            let m = &mut vm.modules.list[id];
            m.dfs_anc = m.dfs_anc.min(anc);
        }
    }
    initialize_environment(vm, id)?;
    if vm.modules.list[id].dfs_anc == vm.modules.list[id].dfs_index {
        while let Some(m) = stack.pop() {
            vm.modules.list[m].status = Status::Linked;
            if m == id {
                break;
            }
        }
    }
    Ok(index)
}

/// InitializeEnvironment: confere os imports e reexportações, cria o
/// ambiente (roda a função do módulo até o InitialYield).
fn initialize_environment(vm: &mut Vm, id: ModuleId) -> Result<(), Value> {
    let m = &vm.modules.list[id];
    for e in &m.indirect_exports {
        let r = resolve_export(&vm.modules, id, &e.name, &mut Vec::new());
        if matches!(r, Resolved::Null | Resolved::Circular | Resolved::Ambiguous) {
            let spec = m.requests[e.req].specifier.clone();
            let name = e.name.clone();
            return Err(link_error(vm, &r, &name, &spec));
        }
    }
    for ie in &m.imports {
        let Some(target) = m.loaded[ie.req] else { continue };
        if let What::Name(n) = &ie.what {
            let r = resolve_export(&vm.modules, target, n, &mut Vec::new());
            match r {
                Resolved::Null | Resolved::Circular | Resolved::Ambiguous => {
                    let spec = m.requests[ie.req].specifier.clone();
                    let n = n.clone();
                    return Err(link_error(vm, &r, &n, &spec));
                }
                Resolved::Source(t) if vm.modules.is_source(t) => return Err(source_unavailable(vm)),
                _ => {}
            }
        }
    }
    // argumentos: registro dos getters, namespaces dos pedidos usados, import.meta
    let used: Vec<(usize, What)> = m.imports.iter().map(|ie| (ie.req, ie.what.clone())).collect();
    let nreq = m.requests.len();
    let url = m.url.clone();
    let Some(code) = m.code.clone() else { return Ok(()) };
    let mut params = vec![Value::Undefined; nreq];
    for (req, what) in used {
        if !params[req].is_undefined() {
            continue;
        }
        let Some(target) = vm.modules.list[id].loaded[req] else { continue };
        let phase = vm.modules.list[id].requests[req].phase;
        params[req] = match (phase, what) {
            (ImportPhase::Source, _) | (_, What::Source) => module_source(vm, target)?,
            (ImportPhase::Defer, _) => Value::Object(namespace(vm, target, true)),
            _ => Value::Object(namespace(vm, target, false)),
        };
    }
    let ex = vm.realm.native("", 0, register_exports);
    set_native_data(&ex, Value::Number(id as f64));
    let meta = ObjRef::new(Object::new(None, Kind::Ordinary));
    if let Some(host) = vm.modules.host {
        (host.meta)(vm, &url, &meta);
    }
    let mut args = Vec::with_capacity(nreq + 2);
    args.push(Value::Object(ex));
    args.extend(params);
    args.push(Value::Object(meta));
    let nregs = code.nregs as usize;
    let mut regs = Vec::with_capacity(nregs);
    for i in 0..nregs {
        regs.push(if i < args.len() {
            args[i].clone()
        } else if i < code.var_end as usize {
            Value::Undefined
        } else {
            Value::Empty
        });
    }
    let mut frame = Box::new(Frame {
        code,
        pc: 0,
        regs,
        stack: Vec::with_capacity(8),
        env: None,
        this: Value::Undefined,
        func: None,
        home: None,
        new_target: Value::Undefined,
        args: Vec::new(),
        handlers: Vec::new(),
        suspended: false,
        suspend_await: false,
    });
    vm.run_frame(&mut frame)?;
    let m = &mut vm.modules.list[id];
    m.frame = Some(frame);
    Ok(())
}

/// `%ex("x", getter, ...)`: guarda os getters das ligações exportadas.
fn register_exports(vm: &mut Vm, a: &mut CallArgs) -> R {
    let Value::Number(id) = native_data(&a.callee) else { return Ok(Value::Undefined) };
    let id = id as usize;
    for pair in a.args.chunks(2) {
        if let [Value::String(name), g] = pair {
            vm.modules.list[id].getters.insert(name.clone(), g.clone());
        }
    }
    Ok(Value::Undefined)
}

/// `import source` de um módulo: o objeto ModuleSource (os módulos de texto
/// JavaScript não têm).
fn module_source(vm: &mut Vm, _id: ModuleId) -> R {
    Err(source_unavailable(vm))
}

fn source_unavailable(vm: &mut Vm) -> Value {
    vm.syntax_error("Source phase import object is not available for the requested module")
}

// ------------------------------------------------------------ avaliação

/// Evaluate(): a promessa da avaliação do módulo (e das dependências).
pub fn evaluate(vm: &mut Vm, id: ModuleId) -> ObjRef {
    let mut module = id;
    if !vm.modules.is_source(module) {
        vm.modules.list[module].status = Status::Evaluated;
        let p = vm.new_promise();
        vm.resolve_promise(&p, Value::Undefined);
        return p;
    }
    if matches!(vm.modules.list[module].status, Status::EvaluatingAsync | Status::Evaluated) {
        module = vm.modules.list[module].cycle_root;
    }
    if let Some(p) = &vm.modules.list[module].tlc {
        return p.clone();
    }
    let p = vm.new_promise();
    vm.modules.list[module].tlc = Some(p.clone());
    let mut stack = Vec::new();
    match inner_evaluate(vm, module, &mut stack, 0) {
        Err(e) => {
            for m in stack {
                let m = &mut vm.modules.list[m];
                m.status = Status::Evaluated;
                m.eval_error = Some(e.clone());
            }
            vm.reject_promise(&p, e);
        }
        Ok(_) => {
            if !vm.modules.list[module].async_evaluating {
                vm.resolve_promise(&p, Value::Undefined);
            }
        }
    }
    p
}

/// InnerModuleEvaluation
fn inner_evaluate(vm: &mut Vm, id: ModuleId, stack: &mut Vec<ModuleId>, mut index: usize) -> Result<usize, Value> {
    if !vm.modules.is_source(id) {
        vm.modules.list[id].status = Status::Evaluated;
        return Ok(index);
    }
    match vm.modules.list[id].status {
        Status::EvaluatingAsync | Status::Evaluated => {
            return match &vm.modules.list[id].eval_error {
                None => Ok(index),
                Some(e) => Err(e.clone()),
            };
        }
        Status::Evaluating => return Ok(index),
        _ => {}
    }
    {
        let m = &mut vm.modules.list[id];
        m.status = Status::Evaluating;
        m.dfs_index = index;
        m.dfs_anc = index;
        m.pending_deps = 0;
    }
    index += 1;
    stack.push(id);
    // a lista do que avaliar: os pedidos, e de cada `import defer` só as
    // dependências assíncronas
    let mut list: Vec<ModuleId> = Vec::new();
    for i in 0..vm.modules.list[id].requests.len() {
        let Some(req) = vm.modules.list[id].loaded[i] else { continue };
        match vm.modules.list[id].requests[i].phase {
            ImportPhase::Defer => {
                for dep in async_dependencies(vm, req) {
                    if !list.contains(&dep) {
                        list.push(dep);
                    }
                }
            }
            ImportPhase::Source => {}
            ImportPhase::Evaluation => {
                if !list.contains(&req) {
                    list.push(req);
                }
            }
        }
    }
    for mut req in list {
        index = inner_evaluate(vm, req, stack, index)?;
        if vm.modules.is_source(req) {
            if vm.modules.list[req].status == Status::Evaluating {
                let anc = vm.modules.list[req].dfs_anc;
                let m = &mut vm.modules.list[id];
                m.dfs_anc = m.dfs_anc.min(anc);
            } else {
                req = vm.modules.list[req].cycle_root;
                if let Some(e) = vm.modules.list[req].eval_error.clone() {
                    return Err(e);
                }
            }
            if vm.modules.list[req].async_evaluating {
                vm.modules.list[id].pending_deps += 1;
                vm.modules.list[req].async_parents.push(id);
            }
        }
    }
    let (pending, tla) = (vm.modules.list[id].pending_deps, vm.modules.list[id].has_tla);
    if pending > 0 || tla {
        vm.modules.order += 1;
        let order = vm.modules.order;
        let m = &mut vm.modules.list[id];
        m.async_evaluating = true;
        m.async_order = order;
        if pending == 0 {
            execute_async(vm, id);
        }
    } else {
        execute_sync(vm, id)?;
    }
    if vm.modules.list[id].dfs_anc == vm.modules.list[id].dfs_index {
        while let Some(m) = stack.pop() {
            let r = &mut vm.modules.list[m];
            r.status = if r.async_evaluating { Status::EvaluatingAsync } else { Status::Evaluated };
            r.cycle_root = id;
            if m == id {
                break;
            }
        }
    }
    Ok(index)
}

/// Roda o corpo de um módulo sem `await` no topo.
fn execute_sync(vm: &mut Vm, id: ModuleId) -> Result<(), Value> {
    let Some(mut frame) = vm.modules.list[id].frame.take() else { return Ok(()) };
    frame.suspended = false;
    vm.run_frame(&mut frame).map(|_| ())
}

/// ExecuteAsyncModule: roda o corpo como função async.
fn execute_async(vm: &mut Vm, id: ModuleId) {
    let p = vm.new_promise();
    let on_ful = vm.realm.native("", 0, async_fulfilled);
    let on_rej = vm.realm.native("", 1, async_rejected);
    set_native_data(&on_ful, Value::Number(id as f64));
    set_native_data(&on_rej, Value::Number(id as f64));
    vm.perform_then(&p, Value::Object(on_ful), Value::Object(on_rej), None);
    match vm.modules.list[id].frame.take() {
        Some(mut frame) => {
            frame.suspended = false;
            let co = ObjRef::new(Object::new(
                None,
                Kind::Async(Box::new(AsyncData {
                    frame: Some(frame),
                    promise: p,
                })),
            ));
            vm.async_step(&co, None);
        }
        None => vm.resolve_promise(&p, Value::Undefined),
    }
}

fn data_id(a: &CallArgs) -> Option<ModuleId> {
    match native_data(&a.callee) {
        Value::Number(n) => Some(n as usize),
        _ => None,
    }
}

fn async_fulfilled(vm: &mut Vm, a: &mut CallArgs) -> R {
    if let Some(id) = data_id(a) {
        async_module_fulfilled(vm, id);
    }
    Ok(Value::Undefined)
}

fn async_rejected(vm: &mut Vm, a: &mut CallArgs) -> R {
    if let Some(id) = data_id(a) {
        async_module_rejected(vm, id, a.arg(0));
    }
    Ok(Value::Undefined)
}

/// GatherAvailableAncestors
fn gather_ancestors(vm: &mut Vm, id: ModuleId, out: &mut Vec<ModuleId>) {
    for m in vm.modules.list[id].async_parents.clone() {
        let root = vm.modules.list[m].cycle_root;
        if !out.contains(&m) && vm.modules.list[root].eval_error.is_none() {
            let r = &mut vm.modules.list[m];
            r.pending_deps = r.pending_deps.saturating_sub(1);
            if r.pending_deps == 0 {
                out.push(m);
                if !r.has_tla {
                    gather_ancestors(vm, m, out);
                }
            }
        }
    }
}

/// AsyncModuleExecutionFulfilled
fn async_module_fulfilled(vm: &mut Vm, id: ModuleId) {
    if vm.modules.list[id].status == Status::Evaluated {
        return;
    }
    {
        let m = &mut vm.modules.list[id];
        m.async_evaluating = false;
        m.status = Status::Evaluated;
    }
    if let Some(p) = vm.modules.list[id].tlc.clone() {
        vm.resolve_promise(&p, Value::Undefined);
    }
    let mut list = Vec::new();
    gather_ancestors(vm, id, &mut list);
    list.sort_by_key(|&m| vm.modules.list[m].async_order);
    for m in list {
        if vm.modules.list[m].status == Status::Evaluated {
            continue;
        }
        if vm.modules.list[m].has_tla {
            execute_async(vm, m);
        } else {
            match execute_sync(vm, m) {
                Err(e) => async_module_rejected(vm, m, e),
                Ok(()) => {
                    {
                        let r = &mut vm.modules.list[m];
                        r.async_evaluating = false;
                        r.status = Status::Evaluated;
                    }
                    if let Some(p) = vm.modules.list[m].tlc.clone() {
                        vm.resolve_promise(&p, Value::Undefined);
                    }
                }
            }
        }
    }
}

/// AsyncModuleExecutionRejected
fn async_module_rejected(vm: &mut Vm, id: ModuleId, error: Value) {
    if vm.modules.list[id].status == Status::Evaluated {
        return;
    }
    {
        let m = &mut vm.modules.list[id];
        m.eval_error = Some(error.clone());
        m.status = Status::Evaluated;
        m.async_evaluating = false;
    }
    // a própria promessa antes das dos ancestrais (da folha para a raiz)
    if let Some(p) = vm.modules.list[id].tlc.clone() {
        vm.reject_promise(&p, error.clone());
    }
    for m in vm.modules.list[id].async_parents.clone() {
        async_module_rejected(vm, m, error.clone());
    }
}

// ------------------------------------------------------------ import defer

/// IsModuleSCCEvaluated: o componente (ciclo) do módulo já terminou?
fn scc_evaluated(vm: &Vm, id: ModuleId) -> bool {
    let root = vm.modules.list[id].cycle_root;
    vm.modules.list[root].status == Status::Evaluated && vm.modules.list[id].status == Status::Evaluated
}

/// GatherAsynchronousTransitiveDependencies: as dependências (do grafo de
/// `id`) com `await` no topo que ainda não terminaram.
fn async_dependencies(vm: &Vm, id: ModuleId) -> Vec<ModuleId> {
    let mut out = Vec::new();
    gather_async(vm, id, &mut Vec::new(), &mut out);
    out
}

fn gather_async(vm: &Vm, id: ModuleId, seen: &mut Vec<ModuleId>, out: &mut Vec<ModuleId>) {
    if seen.contains(&id) {
        return;
    }
    seen.push(id);
    if !vm.modules.is_source(id) {
        return;
    }
    let m = &vm.modules.list[id];
    if m.status == Status::Evaluating || scc_evaluated(vm, id) {
        return;
    }
    if m.has_tla {
        if !out.contains(&id) {
            out.push(id);
        }
        return;
    }
    for (i, r) in m.requests.iter().enumerate() {
        if r.phase == ImportPhase::Source {
            continue;
        }
        if let Some(dep) = m.loaded[i] {
            gather_async(vm, dep, seen, out);
        }
    }
}

/// ReadyForSyncExecution: o módulo pode ser avaliado agora, sem esperar nada?
fn ready_for_sync(vm: &Vm, id: ModuleId, seen: &mut Vec<ModuleId>) -> bool {
    if !vm.modules.is_source(id) || seen.contains(&id) {
        return true;
    }
    seen.push(id);
    if scc_evaluated(vm, id) {
        return true;
    }
    let m = &vm.modules.list[id];
    if matches!(m.status, Status::Evaluating | Status::EvaluatingAsync) || m.has_tla {
        return false;
    }
    m.requests
        .iter()
        .enumerate()
        .filter(|(_, r)| r.phase != ImportPhase::Source)
        .filter_map(|(i, _)| m.loaded[i])
        .all(|dep| ready_for_sync(vm, dep, seen))
}

/// GetModuleExportsList de um namespace adiado: avalia o módulo agora
/// (EvaluateSync), ou TypeError se ele não pode rodar sem esperar.
fn ensure_deferred_evaluated(vm: &mut Vm, id: ModuleId) -> Result<(), Value> {
    if vm.modules.is_source(id) && vm.modules.list[id].status != Status::Evaluated && !ready_for_sync(vm, id, &mut Vec::new()) {
        return Err(vm.type_error("Deferred module is not ready for synchronous evaluation"));
    }
    let p = evaluate(vm, id);
    let rejected = match &p.borrow().kind {
        Kind::Promise(d) => match &d.state {
            crate::builtins::promise::PState::Rejected(e) => Some(e.clone()),
            _ => None,
        },
        _ => None,
    };
    match rejected {
        Some(e) => {
            // o erro sobe como exceção: a promessa não fica "não tratada"
            if let Kind::Promise(d) = &mut p.borrow_mut().kind {
                d.handled = true;
            }
            vm.pending_rejections.retain(|x| !x.ptr_eq(&p));
            Err(e)
        }
        None => Ok(()),
    }
}

// ------------------------------------------------------------ import()

/// Quem está importando: a URL do script ou módulo que está rodando (o
/// código de eval e de `new Function` usa a de quem o chamou).
fn active_referrer(vm: &Vm) -> String {
    for t in vm.trace.iter().rev() {
        let f = &*t.code.file;
        if !f.is_empty() && f != "eval" && f != "anonymous" {
            return f.to_string();
        }
    }
    String::new()
}

/// EvaluateImportCall: `import(especificador, opções)`.
pub fn import_call(vm: &mut Vm, spec: Value, opts: Value, phase: u8) -> Value {
    let p = vm.new_promise();
    let phase = match phase {
        1 => ImportPhase::Defer,
        2 => ImportPhase::Source,
        _ => ImportPhase::Evaluation,
    };
    let referrer = active_referrer(vm);
    match import_request(vm, spec, opts, phase) {
        Ok(req) => host_load(vm, None, &referrer, &req, Payload::Dynamic(p.clone(), phase)),
        Err(e) => vm.reject_promise(&p, e),
    }
    Value::Object(p)
}

/// O pedido de um `import()` (os erros rejeitam a promessa).
fn import_request(vm: &mut Vm, spec: Value, opts: Value, phase: ImportPhase) -> Result<Request, Value> {
    let specifier = vm.to_string(&spec)?.to_rust();
    let mut attrs: ImportAttrs = Vec::new();
    if !opts.is_undefined() {
        if !matches!(opts, Value::Object(_)) {
            return Err(vm.type_error("The second argument to import() must be an object"));
        }
        let with = vm.get_str(&opts, "with")?;
        if !with.is_undefined() {
            let Value::Object(w) = &with else {
                return Err(vm.type_error("The 'with' option must be an object"));
            };
            for k in vm.own_keys(w)? {
                let PropKey::Str(name) = &k else { continue };
                if !vm.get_own_property(w, &k)?.is_some_and(|p| p.enumerable) {
                    continue;
                }
                let v = vm.get_obj(w, &k)?;
                let Value::String(s) = v else {
                    return Err(vm.type_error("Import attribute value must be a string"));
                };
                attrs.push((name.to_rust(), s.to_rust()));
            }
        }
        if let Err(msg) = ModType::from_attrs(&attrs) {
            return Err(vm.type_error(msg));
        }
    }
    attrs.sort();
    Ok(Request { specifier, attrs, phase })
}

/// ContinueDynamicImport
fn continue_dynamic_import(vm: &mut Vm, p: ObjRef, phase: ImportPhase, result: Result<ModuleId, Value>) {
    let id = match result {
        Ok(id) => id,
        Err(e) => return vm.reject_promise(&p, e),
    };
    if phase == ImportPhase::Source {
        let r = module_source(vm, id);
        match r {
            Ok(v) => vm.resolve_promise(&p, v),
            Err(e) => vm.reject_promise(&p, e),
        }
        return;
    }
    let load = vm.new_promise();
    load_requested(vm, id, GraphDone::Promise(load.clone()));
    let data = vm.array_value(vec![Value::Object(p), Value::Number(id as f64), Value::Bool(phase == ImportPhase::Defer)]);
    let on_rej = vm.realm.native("", 1, import_rejected);
    set_native_data(&on_rej, data.clone());
    let link_eval = vm.realm.native("", 0, import_link_evaluate);
    set_native_data(&link_eval, data);
    vm.perform_then(&load, Value::Object(link_eval), Value::Object(on_rej), None);
}

/// (promessa do import, módulo, adiado) guardados numa função interna.
fn import_data(a: &CallArgs) -> Option<(ObjRef, ModuleId, bool, Value)> {
    let data = native_data(&a.callee);
    let Value::Object(arr) = &data else { return None };
    let b = arr.borrow();
    let Kind::Array(v) = &b.kind else { return None };
    match (v.first(), v.get(1), v.get(2)) {
        (Some(Value::Object(p)), Some(Value::Number(id)), Some(Value::Bool(d))) => Some((p.clone(), *id as usize, *d, data.clone())),
        _ => None,
    }
}

fn import_rejected(vm: &mut Vm, a: &mut CallArgs) -> R {
    if let Some((p, ..)) = import_data(a) {
        vm.reject_promise(&p, a.arg(0));
    }
    Ok(Value::Undefined)
}

fn import_link_evaluate(vm: &mut Vm, a: &mut CallArgs) -> R {
    let Some((p, id, deferred, data)) = import_data(a) else { return Ok(Value::Undefined) };
    if let Err(e) = link(vm, id) {
        vm.reject_promise(&p, e);
        return Ok(Value::Undefined);
    }
    let on_rej = vm.realm.native("", 1, import_rejected);
    set_native_data(&on_rej, data.clone());
    let on_ful = vm.realm.native("", 0, import_fulfilled);
    set_native_data(&on_ful, data);
    let ev = if deferred {
        // `import.defer`: só as dependências assíncronas rodam agora
        let deps = async_dependencies(vm, id);
        if deps.is_empty() {
            let ns = namespace(vm, id, true);
            vm.resolve_promise(&p, Value::Object(ns));
            return Ok(Value::Undefined);
        }
        // espera todas, sem passar por Promise.prototype.then
        let all = vm.new_promise();
        let count = vm.array_value(vec![Value::Number(deps.len() as f64), Value::Object(all.clone())]);
        for d in deps {
            let pd = evaluate(vm, d);
            let one = vm.realm.native("", 1, all_one);
            set_native_data(&one, count.clone());
            let fail = vm.realm.native("", 1, all_fail);
            set_native_data(&fail, count.clone());
            vm.perform_then(&pd, Value::Object(one), Value::Object(fail), None);
        }
        all
    } else {
        evaluate(vm, id)
    };
    vm.perform_then(&ev, Value::Object(on_ful), Value::Object(on_rej), None);
    Ok(Value::Undefined)
}

/// [restantes, promessa] de uma espera por várias avaliações.
fn all_state(a: &CallArgs) -> Option<(ObjRef, ObjRef)> {
    let Value::Object(arr) = native_data(&a.callee) else { return None };
    let p = match &arr.borrow().kind {
        Kind::Array(v) => match v.get(1) {
            Some(Value::Object(p)) => p.clone(),
            _ => return None,
        },
        _ => return None,
    };
    Some((arr, p))
}

fn all_one(vm: &mut Vm, a: &mut CallArgs) -> R {
    if let Some((arr, p)) = all_state(a) {
        let left = match &mut arr.borrow_mut().kind {
            Kind::Array(v) => match v.first_mut() {
                Some(Value::Number(n)) => {
                    *n -= 1.0;
                    *n
                }
                _ => 1.0,
            },
            _ => 1.0,
        };
        if left == 0.0 {
            vm.resolve_promise(&p, Value::Undefined);
        }
    }
    Ok(Value::Undefined)
}

fn all_fail(vm: &mut Vm, a: &mut CallArgs) -> R {
    if let Some((_, p)) = all_state(a) {
        vm.reject_promise(&p, a.arg(0));
    }
    Ok(Value::Undefined)
}

fn import_fulfilled(vm: &mut Vm, a: &mut CallArgs) -> R {
    if let Some((p, id, deferred, _)) = import_data(a) {
        let ns = namespace(vm, id, deferred);
        vm.resolve_promise(&p, Value::Object(ns));
    }
    Ok(Value::Undefined)
}

// ------------------------------------------------------------ utilidades

impl Vm {
    /// O namespace do módulo dessa URL (já carregado), para o hospedeiro.
    pub fn module_namespace(&mut self, url: &str) -> Option<Value> {
        let id = self.modules.find(url)?;
        Some(Value::Object(namespace(self, id, false)))
    }
}
