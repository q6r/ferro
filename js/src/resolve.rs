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

