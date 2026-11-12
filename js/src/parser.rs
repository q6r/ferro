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

pub struct Parser {
    lx: Lexer,
    tok: Token,
    prev_end: usize,
    prev_line: u32,
    ctx: Ctx,
    strict: bool,
    /// Analisando um módulo ES (aceita `import.meta`).
    module: bool,
}

const RESERVED: &[&str] = &[
    "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export",
    "extends", "finally", "for", "function", "if", "import", "in", "instanceof", "new", "return", "super", "switch",
    "this", "throw", "try", "typeof", "var", "void", "while", "with", "null", "true", "false", "enum",
];

fn bin_prec(t: &Tok, no_in: bool) -> Option<(u8, BinOrLog)> {
    let p = match t {
        Tok::Punct(p) => *p,
        Tok::Ident(n) if &**n == "instanceof" => return Some((8, BinOrLog::Bin(BinOp::InstanceOf))),
        Tok::Ident(n) if &**n == "in" && !no_in => return Some((8, BinOrLog::Bin(BinOp::In))),
        _ => return None,
    };
    Some(match p {
        "??" => (1, BinOrLog::Log(LogOp::Nullish)),
        "||" => (2, BinOrLog::Log(LogOp::Or)),
        "&&" => (3, BinOrLog::Log(LogOp::And)),
        "|" => (4, BinOrLog::Bin(BinOp::BitOr)),
        "^" => (5, BinOrLog::Bin(BinOp::BitXor)),
        "&" => (6, BinOrLog::Bin(BinOp::BitAnd)),
        "==" => (7, BinOrLog::Bin(BinOp::Eq)),
        "!=" => (7, BinOrLog::Bin(BinOp::Ne)),
        "===" => (7, BinOrLog::Bin(BinOp::StrictEq)),
        "!==" => (7, BinOrLog::Bin(BinOp::StrictNe)),
        "<" => (8, BinOrLog::Bin(BinOp::Lt)),
        ">" => (8, BinOrLog::Bin(BinOp::Gt)),
        "<=" => (8, BinOrLog::Bin(BinOp::Le)),
        ">=" => (8, BinOrLog::Bin(BinOp::Ge)),
        "<<" => (9, BinOrLog::Bin(BinOp::Shl)),
        ">>" => (9, BinOrLog::Bin(BinOp::Shr)),
        ">>>" => (9, BinOrLog::Bin(BinOp::UShr)),
        "+" => (10, BinOrLog::Bin(BinOp::Add)),
        "-" => (10, BinOrLog::Bin(BinOp::Sub)),
        "*" => (11, BinOrLog::Bin(BinOp::Mul)),
        "/" => (11, BinOrLog::Bin(BinOp::Div)),
        "%" => (11, BinOrLog::Bin(BinOp::Mod)),
        "**" => (12, BinOrLog::Bin(BinOp::Exp)),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
enum BinOrLog {
    Bin(BinOp),
    Log(LogOp),
}

fn assign_op(p: &str) -> Option<AssignOp> {
    Some(match p {
        "=" => AssignOp::Assign,
        "+=" => AssignOp::Op(BinOp::Add),
        "-=" => AssignOp::Op(BinOp::Sub),
        "*=" => AssignOp::Op(BinOp::Mul),
        "/=" => AssignOp::Op(BinOp::Div),
        "%=" => AssignOp::Op(BinOp::Mod),
        "**=" => AssignOp::Op(BinOp::Exp),
        "<<=" => AssignOp::Op(BinOp::Shl),
        ">>=" => AssignOp::Op(BinOp::Shr),
        ">>>=" => AssignOp::Op(BinOp::UShr),
        "&=" => AssignOp::Op(BinOp::BitAnd),
        "|=" => AssignOp::Op(BinOp::BitOr),
        "^=" => AssignOp::Op(BinOp::BitXor),
        "&&=" => AssignOp::Logical(LogOp::And),
        "||=" => AssignOp::Logical(LogOp::Or),
        "??=" => AssignOp::Logical(LogOp::Nullish),
        _ => return None,
    })
}

pub fn parse_program(src: &str) -> PResult<Program> {
    let mut p = Parser::new(src)?;
    let body = p.parse_body_until_eof()?;
    Ok(Program { body, strict: p.strict, imports: Default::default() })
}

/// Um módulo ES: instruções, `import` e `export` (sempre estrito, com
/// `await` no topo).
pub fn parse_module(src: &str) -> PResult<Vec<ModuleItem>> {
    let mut p = Parser::new(src)?;
    p.strict = true;
    p.module = true;
    p.ctx.in_async = true;
    let mut items = Vec::new();
    while p.tok.tok != Tok::Eof {
        if p.is_kw("import") && !matches!(p.peek()?.tok, Tok::Punct("(") | Tok::Punct(".")) {
            items.push(ModuleItem::Import(p.parse_import()?));
        } else if p.is_kw("export") {
            items.push(ModuleItem::Export(p.parse_export()?));
        } else {
            items.push(ModuleItem::Stmt(p.parse_statement()?));
        }
    }
    Ok(items)
}

