//! Analisador léxico do JavaScript.
//!
//! A divisão `/` versus o início de uma regex depende do token anterior, e
//! a continuação de um template (`}` dentro de `${...}`) depende do parser;
//! por isso o parser pode pedir para reler a partir de uma posição.

use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// Identificadores e palavras reservadas (o parser decide o papel).
    Ident(Rc<str>),
    /// `#nome` (campos privados).
    Private(Rc<str>),
    Num(f64),
    BigInt(Rc<str>),
    Str(Rc<[u16]>),
    /// Pedaço de template: texto "cozido" (None se tiver escape inválido),
    /// texto cru, e se é o último pedaço.
    Template {
        cooked: Option<Rc<[u16]>>,
        raw: Rc<str>,
        tail: bool,
    },
    Regex {
        pattern: Rc<str>,
        flags: Rc<str>,
    },
    Punct(&'static str),
    Eof,
}

