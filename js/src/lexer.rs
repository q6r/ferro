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

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
    pub line: u32,
    /// Houve quebra de linha antes deste token (inserção automática de `;`).
    pub nl_before: bool,
    /// Identificador escrito com escapes (não pode ser palavra reservada).
    pub escaped: bool,
}


pub struct Lexer {
    src: Vec<char>,
    pos: usize,
    line: u32,
    /// O último token permite que `/` comece uma regex?
    regex_ok: bool,
    /// Identificadores já vistos: o mesmo nome reaproveita o mesmo Rc<str>.
    names: crate::fxhash::FxHashMap<Box<str>, Rc<str>>,
    buf: String,
}

#[derive(Debug, Clone)]
pub struct LexError {
    pub msg: String,
    pub line: u32,
}

fn is_id_start(c: char) -> bool {
    c.is_alphabetic() || c == '$' || c == '_' || (c as u32 > 0x7f && !c.is_whitespace() && !c.is_control())
}

fn is_id_part(c: char) -> bool {
    is_id_start(c) || c.is_ascii_digit() || c == '\u{200C}' || c == '\u{200D}'
}

fn is_line_term(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Depois deste token, uma `/` começa uma regex (e não uma divisão)?
pub fn regex_allowed_after(tok: &Tok) -> bool {
    match tok {
        Tok::Ident(n) => matches!(
            &**n,
            "return" | "typeof" | "instanceof" | "in" | "of" | "new" | "delete" | "void" | "throw" | "case" | "do"
                | "else" | "yield" | "await"
        ),
        // depois de "}" assumimos início de instrução (regex permitida)
        Tok::Punct("}") => true,
        Tok::Punct(p) => !matches!(*p, ")" | "]" | "++" | "--"),
        Tok::Template { tail, .. } => !*tail,
        _ => false,
    }
}

