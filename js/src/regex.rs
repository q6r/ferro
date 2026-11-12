//! Motor de expressões regulares do JavaScript.
//!
//! Padrão (sintaxe ECMAScript, com as regras legadas do Anexo B) → árvore →
//! bytecode → máquina de backtracking com pilha explícita (sem recursão, para
//! não estourar a pilha com `.*` em textos grandes). Repetições de um único
//! caractere (`a*`, `[a-z]+`, `.*?`) têm instrução própria, que guarda um só
//! ponto de retorno em vez de um por caractere.
//!
//! O texto é UTF-16 (como as strings do JS). Sem a flag `u`, casa unidades;
//! com `u`/`v`, casa pontos de código (pares substitutos juntos).

use std::collections::HashMap;

pub const NONE: usize = usize::MAX;
const INF: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub struct RegexError(pub String);

#[derive(Clone, Copy, Default, Debug)]
pub struct Flags {
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub unicode: bool,
    pub sticky: bool,
    pub has_indices: bool,
    pub unicode_sets: bool,
}

impl Flags {
