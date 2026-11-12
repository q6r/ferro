//! Parser HTML: tokenizador + construtor de árvore.
//!
//! Segue a ideia do algoritmo WHATWG de forma simplificada: trata os casos
//! que aparecem de verdade na web (tags sem fechamento, <p> e <li> fechados
//! implicitamente, <script>/<style> como texto bruto, entidades), mas ainda
//! não implementa todos os modos de inserção (tabelas, adoption agency).

use crate::dom::{Document, ElementData, NodeData, NodeId};

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Doctype(String),
    StartTag {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    EndTag {
        name: String,
    },
    Text(String),
    Comment(String),
}

// ---------------------------------------------------------------- tokenizador

/// Scripts ligados? Com eles, `<noscript>` vira texto bruto (e não aparece).
static SCRIPTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_scripting(on: bool) {
    SCRIPTING.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn scripting() -> bool {
    SCRIPTING.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn tokenize(input: &str) -> Vec<Token> {
    let scripting = scripting();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut i = 0;

