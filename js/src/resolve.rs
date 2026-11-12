//! Análise de capturas: descobre quais declarações são usadas por funções
//! internas (e por isso precisam morar num ambiente no heap).
//!
//! Espelha exatamente os escopos que o compilador cria. Cada escopo é
//! identificado pelo endereço do nó da AST que o cria (a AST não muda
//! durante a compilação), e o resultado é o conjunto de pares
//! (escopo, nome) capturados.

use crate::fxhash::FxHashSet;

use crate::ast::*;
use crate::compiler::{bound_names, lexical_decls, var_names};

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
}

pub struct Resolver {
    scopes: Vec<Scope>,
    func: u32,
    strict: Vec<bool>,
    pub out: Captures,
    /// Importações do módulo: o nome local lê o namespace (que é capturado).
    imports: crate::fxhash::FxHashMap<Name, (Name, Option<Name>)>,
}

impl Resolver {
    pub fn new(strict: bool) -> Resolver {
        Resolver {
            scopes: Vec::new(),
            func: 0,
            strict: vec![strict],
            out: FxHashSet::default(),
            imports: Default::default(),
        }
    }

    fn strict(&self) -> bool {
        *self.strict.last().unwrap_or(&false)
    }

    fn push(&mut self, key: usize, names: Vec<Name>) {
        self.scopes.push(Scope {
            key,
            func: self.func,
            names: names.into_iter().collect(),
        });
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn refer(&mut self, n: &Name) {
        for sc in self.scopes.iter().rev() {
            if sc.names.contains(n) {
                if sc.func < self.func {
                    self.out.insert((sc.key, n.clone()));
                }
                return;
            }
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
        var_names(body, &mut names, !strict, false);
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

    fn decls(&mut self, decls: &[VarDecl]) {
        for d in decls {
            if let Some(i) = &d.init {
                self.expr(i);
            }
            self.pattern(&d.target);
        }
    }

