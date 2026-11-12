//! Parser do JavaScript (ES2023): descendente recursivo, com precedência de
//! operadores e retrocesso para distinguir arrow functions de parênteses.

use std::rc::Rc;

use crate::ast::*;
use crate::lexer::{LexError, Lexer, Tok, Token};

#[derive(Debug, Clone)]
pub struct ParseError {
    pub msg: String,
    pub line: u32,
    /// O erro aconteceu no fim do texto (entrada incompleta, útil no REPL).
    pub eof: bool,
}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> ParseError {
        let eof = e.msg.contains("sem fim");
        ParseError {
            msg: e.msg,
            line: e.line,
            eof,
        }
    }
}

type PResult<T> = Result<T, ParseError>;

#[derive(Clone, Copy, Default)]
struct Ctx {
    in_function: bool,
    in_async: bool,
    in_generator: bool,
    in_class: bool,
}

