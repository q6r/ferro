//! Análise de capturas: descobre quais declarações são usadas por funções
//! internas (e por isso precisam morar num ambiente no heap).
//!
//! Espelha exatamente os escopos que o compilador cria. Cada escopo é
//! identificado pelo endereço do nó da AST que o cria (a AST não muda
//! durante a compilação), e o resultado é o conjunto de pares
//! (escopo, nome) capturados.

use crate::fxhash::FxHashSet;

use crate::ast::*;
use crate::compiler::{annex_b_functions, bound_names, lexical_decls, var_names};

pub type Captures = FxHashSet<(usize, Name)>;

pub fn key_of<T>(r: &T) -> usize {
    r as *const T as usize
}

pub fn slice_key(s: &[Stmt]) -> usize {
    s.as_ptr() as usize
}

/// Escopo da função sintética de campos de instância / estáticos de uma classe.
pub fn class_init_key(c: &ClassDef, is_static: bool) -> usize {
    key_of(c) + if is_static { 2 } else { 1 }
}

struct Scope {
    key: usize,
    func: u32,
    names: FxHashSet<Name>,
    /// Escopo de um `with`: todo nome livre lá dentro consulta o objeto.
    is_with: bool,
}

/// Nome da variável escondida que guarda o objeto de um `with`.
pub const WITH_OBJ: &str = "%with";

/// Nome da variável escondida com as `var` criadas por `eval` direto
/// (não estrito) dentro de uma função.
pub const EVAL_VARS: &str = "%vars";

/// `eval(...)` com o identificador `eval` puro (sem `?.`): pode ser eval direto.
pub fn is_direct_eval(callee: &Expr, optional: bool) -> bool {
    !optional && matches!(callee, Expr::Ident(n, _) if &**n == "eval")
}

pub struct Resolver {
    scopes: Vec<Scope>,
    func: u32,
    strict: Vec<bool>,
    pub out: Captures,
    /// Importações do módulo: o nome local lê o namespace (que é capturado).
    imports: crate::fxhash::FxHashMap<Name, (Name, Option<Name>)>,
    /// Escopos que envolvem um `eval` direto: o texto do eval pode usar
    /// qualquer nome deles, então tudo ali mora no heap.
    pub evals: FxHashSet<usize>,
    /// Funções (chave do escopo) com `eval` direto no próprio corpo.
    pub eval_fns: FxHashSet<usize>,
}

impl Resolver {
    pub fn new(strict: bool) -> Resolver {
        Resolver {
            scopes: Vec::new(),
            func: 0,
            strict: vec![strict],
            out: FxHashSet::default(),
            imports: Default::default(),
            evals: FxHashSet::default(),
            eval_fns: FxHashSet::default(),
        }
    }

    /// Um `eval` direto aqui: marca todos os escopos em volta.
    fn saw_eval(&mut self) {
        for sc in &self.scopes {
            self.evals.insert(sc.key);
        }
        if self.func > 0
            && let Some(sc) = self.scopes.iter().find(|sc| sc.func == self.func)
        {
            self.eval_fns.insert(sc.key);
        }
    }

    /// Código de um `eval` direto: um corpo de função com as declarações
    /// próprias (as `var` de um eval não estrito ficam fora dele).
    pub fn eval_body(&mut self, body: &[Stmt], key: usize, strict: bool) {
        self.func += 1;
        self.strict.push(strict);
        let mut names: Vec<Name> = Vec::new();
        if strict {
            var_names(body, &mut names, false, false);
            for s in body {
                if let Stmt::Function(fd) = s
                    && let Some(n) = &fd.name
                {
                    names.push(n.clone());
                }
            }
        }
        let mut lex = Vec::new();
        lexical_decls(body, false, &mut lex);
        names.extend(lex.into_iter().map(|(n, _)| n));
        self.push(key, names);
        self.stmts(body);
        self.pop();
        self.strict.pop();
        self.func -= 1;
    }

    fn strict(&self) -> bool {
        *self.strict.last().unwrap_or(&false)
    }

    fn push(&mut self, key: usize, names: Vec<Name>) {
        self.scopes.push(Scope {
            key,
            func: self.func,
            names: names.into_iter().collect(),
            is_with: false,
        });
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn refer(&mut self, n: &Name) {
        let mut found = None;
        for sc in self.scopes.iter().rev() {
            if sc.names.contains(n) {
                if sc.func < self.func {
                    found = Some((sc.key, n.clone()));
                }
                break;
            }
            // passou por um `with`: a função interna também lê o objeto dele
            if sc.is_with && sc.func < self.func {
                self.out.insert((sc.key, WITH_OBJ.into()));
            }
        }
        if let Some(k) = found {
            self.out.insert(k);
            return;
        }
        if self.scopes.iter().any(|sc| sc.names.contains(n)) {
            return;
        }
        if let Some((ns, _)) = self.imports.get(n).cloned() {
            self.refer(&ns);
        }
    }

    pub fn program(&mut self, p: &Program) {
        self.imports = p.imports.clone();
        // nomes do topo do script são globais: não precisam de análise
        self.stmts(&p.body);
    }

    pub fn function(&mut self, f: &FunctionDef, self_binding: bool) {
        let strict = f.strict || self.strict();
        self.func += 1;
        self.strict.push(strict);
        let mut names: Vec<Name> = Vec::new();
        if self_binding && let Some(n) = &f.name {
            names.push(n.clone());
        }
        if f.kind != FnKind::Arrow {
            names.push("arguments".into());
        }
        for p in &f.params {
            bound_names(p, &mut names);
        }
        if let Some(r) = &f.rest {
            bound_names(r, &mut names);
        }
        let body: &[Stmt] = match &f.body {
            FnBody::Block(b) => b,
            FnBody::Expr(_) => &[],
        };
        var_names(body, &mut names, false, false);
        if !strict {
            // mesmas funções em bloco que o compilador transforma em `var`
            let mut top = Vec::new();
            for p in &f.params {
                bound_names(p, &mut top);
            }
            if let Some(r) = &f.rest {
                bound_names(r, &mut top);
            }
            let mut lex = Vec::new();
            lexical_decls(body, false, &mut lex);
            top.extend(lex.into_iter().map(|(n, _)| n));
            names.extend(annex_b_functions(body, top).into_iter().map(|(n, _)| n));
        }
        for s in body {
            if let Stmt::Function(fd) = s
                && let Some(n) = &fd.name
            {
                names.push(n.clone());
            }
        }
        let mut lex = Vec::new();
        lexical_decls(body, false, &mut lex);
        names.extend(lex.into_iter().map(|(n, _)| n));
        self.push(key_of(f), names);
        for p in &f.params {
            self.pattern(p);
        }
        if let Some(r) = &f.rest {
            self.pattern(r);
        }
        match &f.body {
            FnBody::Block(b) => self.stmts(b),
            FnBody::Expr(e) => self.expr(e),
        }
        self.pop();
        self.strict.pop();
        self.func -= 1;
    }

    fn stmts(&mut self, ss: &[Stmt]) {
        for s in ss {
            self.stmt(s);
        }
    }

    fn block(&mut self, ss: &[Stmt], key: usize) {
        let mut lex = Vec::new();
        lexical_decls(ss, true, &mut lex);
        self.push(key, lex.into_iter().map(|(n, _)| n).collect());
        self.stmts(ss);
        self.pop();
    }

    /// Ramo de um if: `if (x) function f() {}` vale como `{ function f() {} }`.
    fn branch(&mut self, s: &Stmt) {
        if let Stmt::Function(_) = s {
            self.block(std::slice::from_ref(s), key_of(s));
        } else {
            self.stmt(s);
        }
    }

    fn decls(&mut self, decls: &[VarDecl]) {
        for d in decls {
            if let Some(i) = &d.init {
                self.expr(i);
            }
            self.pattern(&d.target);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Expr(e, _) | Stmt::Throw(e, _) => self.expr(e),
            Stmt::Var(_, decls, _) => self.decls(decls),
            Stmt::Function(f) => self.function(f, false),
            Stmt::Class(c) => self.class(c),
            Stmt::Return(e, _) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            Stmt::If(t, c, a) => {
                self.expr(t);
                self.branch(c);
                if let Some(a) = a {
                    self.branch(a);
                }
            }
            Stmt::Block(b) => self.block(b, slice_key(b)),
            Stmt::For { init, test, update, body } => {
                let scoped = matches!(init, Some(ForInit::Var(VarKind::Let | VarKind::Const, _)));
                if let (true, Some(ForInit::Var(_, decls))) = (scoped, init) {
                    let mut names = Vec::new();
                    for d in decls {
                        bound_names(&d.target, &mut names);
                    }
                    self.push(key_of(init), names);
                }
                match init {
                    Some(ForInit::Var(_, decls)) => self.decls(decls),
                    Some(ForInit::Expr(e)) => self.expr(e),
                    None => {}
                }
                if let Some(t) = test {
                    self.expr(t);
                }
                if let Some(u) = update {
                    self.expr(u);
                }
                self.stmt(body);
                if scoped {
                    self.pop();
                }
            }
            Stmt::ForIn { left, right, body } | Stmt::ForOf { left, right, body, .. } => {
                self.expr(right);
                match left {
                    ForHead::Var(VarKind::Let | VarKind::Const, p) => {
                        let mut names = Vec::new();
                        bound_names(p, &mut names);
                        self.push(key_of(left), names);
                        self.pattern(p);
                        self.stmt(body);
                        self.pop();
                    }
                    ForHead::Var(_, p) | ForHead::Pattern(p) => {
                        self.pattern(p);
                        self.stmt(body);
                    }
                }
            }
            Stmt::While(t, b) | Stmt::DoWhile(b, t) => {
                self.expr(t);
                self.stmt(b);
            }
            Stmt::Try {
                block,
                param,
                handler,
                finalizer,
            } => {
                self.block(block, slice_key(block));
                if let Some(h) = handler {
                    let mut names = Vec::new();
                    if let Some(p) = param {
                        bound_names(p, &mut names);
                    }
                    let key = param.as_ref().map(key_of).unwrap_or(0);
                    self.push(key, names);
                    if let Some(p) = param {
                        self.pattern(p);
                    }
                    self.block(h, slice_key(h));
                    self.pop();
                }
                if let Some(f) = finalizer {
                    self.block(f, slice_key(f));
                }
            }
            Stmt::Switch(disc, cases) => {
                self.expr(disc);
                let mut lex = Vec::new();
                for c in cases.iter() {
                    lexical_decls(&c.body, true, &mut lex);
                }
                self.push(cases.as_ptr() as usize, lex.into_iter().map(|(n, _)| n).collect());
                for c in cases {
                    if let Some(t) = &c.test {
                        self.expr(t);
                    }
                    self.stmts(&c.body);
                }
                self.pop();
            }
            Stmt::Labeled(_, b) => self.stmt(b),
            Stmt::With(obj, body) => {
                self.expr(obj);
                self.push(key_of(&**body), vec![WITH_OBJ.into()]);
                if let Some(sc) = self.scopes.last_mut() {
                    sc.is_with = true;
                }
                self.stmt(body);
                self.pop();
            }
            Stmt::Break(..) | Stmt::Continue(..) | Stmt::Empty | Stmt::Debugger => {}
        }
    }

    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Ident(n) => self.refer(n),
            Pattern::Object { props, rest } => {
                for (k, v) in props {
                    if let PropKeyExpr::Computed(e) = k {
                        self.expr(e);
                    }
                    self.pattern(v);
                }
                if let Some(r) = rest {
                    self.pattern(r);
                }
            }
            Pattern::Array { elems, rest } => {
                for p in elems.iter().flatten() {
                    self.pattern(p);
                }
                if let Some(r) = rest {
                    self.pattern(r);
                }
            }
            Pattern::Default(p, e) => {
                self.expr(e);
                self.pattern(p);
            }
            Pattern::Target(e) => self.expr(e),
        }
    }

    fn class(&mut self, c: &ClassDef) {
        self.strict.push(true);
        let names = c.name.iter().cloned().collect();
        self.push(key_of(c), names);
        if let Some(s) = &c.superclass {
            self.expr(s);
        }
        if let Some(f) = &c.constructor {
            self.function(f, false);
        }
        for m in &c.members {
            if let PropKeyExpr::Computed(e) = &m.key {
                self.expr(e);
            }
            if !matches!(m.key, PropKeyExpr::Private(_))
                && let Some(f) = &m.func
            {
                self.function(f, false);
            }
        }
        // funções sintéticas de campos: instância e estáticos
        for is_static in [false, true] {
            let ms: Vec<&ClassMember> = c
                .members
                .iter()
                .filter(|m| m.is_static == is_static)
                .filter(|m| matches!(m.kind, MemberKind::Field(_) | MemberKind::StaticBlock(_)) || matches!(m.key, PropKeyExpr::Private(_)))
                .collect();
            if ms.is_empty() {
                continue;
            }
            self.func += 1;
            let mut names: Vec<Name> = Vec::new();
            for m in &ms {
                if let MemberKind::StaticBlock(b) = &m.kind {
                    var_names(b, &mut names, false, false);
                }
            }
            self.push(class_init_key(c, is_static), names);
            for m in &ms {
                match &m.kind {
                    MemberKind::Field(Some(e)) => self.expr(e),
                    MemberKind::StaticBlock(b) => self.block(b, slice_key(b)),
                    _ => {}
                }
                if matches!(m.key, PropKeyExpr::Private(_))
                    && let Some(f) = &m.func
                {
                    self.function(f, false);
                }
            }
            self.pop();
            self.func -= 1;
        }
        self.pop();
        self.strict.pop();
    }

    fn elems(&mut self, es: &[ArrayElem]) {
        for e in es {
            match e {
                ArrayElem::Expr(e) | ArrayElem::Spread(e) => self.expr(e),
                ArrayElem::Hole => {}
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Ident(n, _) => self.refer(n),
            Expr::Num(_) | Expr::BigInt(_) | Expr::Str(_) | Expr::Regex(..) | Expr::Null | Expr::Bool(_) | Expr::This | Expr::NewTarget | Expr::Unsupported(_) => {}
            Expr::ImportCall(e) => self.expr(e),
            Expr::ImportMeta => self.refer(&Name::from("__meta")),
            Expr::SuperMember(_) => {}
            Expr::SuperIndex(i) => self.expr(i),
            Expr::SuperCall(args) => self.elems(args),
            Expr::Template(_, es) => {
                for e in es {
                    self.expr(e);
                }
            }
            Expr::Tagged { tag, exprs, .. } => {
                self.expr(tag);
                for e in exprs {
                    self.expr(e);
                }
            }
            Expr::Array(es) => self.elems(es),
            Expr::Object(props) => {
                for p in props {
                    if let PropKeyExpr::Computed(k) = &p.key {
                        self.expr(k);
                    }
                    self.expr(&p.value);
                }
            }
            Expr::Function(f) => {
                let self_binding = f.name.is_some() && f.kind == FnKind::Normal;
                self.function(f, self_binding);
            }
            Expr::Class(c) => self.class(c),
            Expr::Unary(_, x) | Expr::Await(x) | Expr::Spread(x) | Expr::OptionalChain(x) => self.expr(x),
            Expr::Update { target, .. } => self.expr(target),
            Expr::Binary(_, l, r) | Expr::Logical(_, l, r) => {
                self.expr(l);
                self.expr(r);
            }
            Expr::Assign(_, p, v) => {
                self.expr(v);
                self.pattern(p);
            }
            Expr::Cond(t, c, a) => {
                self.expr(t);
                self.expr(c);
                self.expr(a);
            }
            Expr::Call { callee, args, optional, .. } => {
                if is_direct_eval(callee, *optional) {
                    self.saw_eval();
                }
                self.expr(callee);
                self.elems(args);
            }
            Expr::New(c, args, _) => {
                self.expr(c);
                self.elems(args);
            }
            Expr::Member { obj, .. } | Expr::PrivateMember(obj, _) | Expr::PrivateIn(_, obj) => self.expr(obj),
            Expr::Index { obj, index, .. } => {
                self.expr(obj);
                self.expr(index);
            }
            Expr::Seq(es) => {
                for e in es {
                    self.expr(e);
                }
            }
            Expr::Yield(x, _) => {
                if let Some(x) = x {
                    self.expr(x);
                }
            }
        }
    }
}
