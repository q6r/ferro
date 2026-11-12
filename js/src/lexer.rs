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

impl Lexer {
    pub fn new(src: &str) -> Lexer {
        Lexer {
            src: src.chars().collect(),
            pos: 0,
            line: 1,
            regex_ok: true,
            names: Default::default(),
            buf: String::new(),
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn line(&self) -> u32 {
        self.line
    }

    /// Volta para uma posição já vista (o parser usa para reler templates).
    pub fn reset(&mut self, pos: usize, line: u32, regex_ok: bool) {
        self.pos = pos;
        self.line = line;
        self.regex_ok = regex_ok;
    }

    pub fn source_slice(&self, start: usize, end: usize) -> String {
        self.src[start.min(self.src.len())..end.min(self.src.len())].iter().collect()
    }

    fn peek(&self) -> char {
        self.src.get(self.pos).copied().unwrap_or('\0')
    }

    fn peek_at(&self, k: usize) -> char {
        self.src.get(self.pos + k).copied().unwrap_or('\0')
    }

    fn err(&self, msg: impl Into<String>) -> LexError {
        LexError {
            msg: msg.into(),
            line: self.line,
        }
    }

    /// Pula espaços e comentários; devolve se passou por uma quebra de linha.
    fn skip_trivia(&mut self) -> Result<bool, LexError> {
        let mut nl = false;
        loop {
            let c = self.peek();
            if c == '\0' && self.pos >= self.src.len() {
                return Ok(nl);
            }
            if is_line_term(c) {
                if c == '\r' && self.peek_at(1) == '\n' {
                    self.pos += 1;
                }
                self.pos += 1;
                self.line += 1;
                nl = true;
            } else if c.is_whitespace() || c == '\u{FEFF}' {
                self.pos += 1;
            } else if c == '/' && self.peek_at(1) == '/' {
                while self.pos < self.src.len() && !is_line_term(self.peek()) {
                    self.pos += 1;
                }
            } else if c == '/' && self.peek_at(1) == '*' {
                self.pos += 2;
                loop {
                    if self.pos >= self.src.len() {
                        return Err(self.err("comentário sem fim"));
                    }
                    let d = self.peek();
                    if d == '*' && self.peek_at(1) == '/' {
                        self.pos += 2;
                        break;
                    }
                    if is_line_term(d) {
                        self.line += 1;
                        nl = true;
                    }
                    self.pos += 1;
                }
            } else if c == '<' && self.peek_at(1) == '!' && self.peek_at(2) == '-' && self.peek_at(3) == '-' {
                // comentário estilo HTML (legado)
                while self.pos < self.src.len() && !is_line_term(self.peek()) {
                    self.pos += 1;
                }
            } else if c == '#' && self.peek_at(1) == '!' && self.pos == 0 {
                while self.pos < self.src.len() && !is_line_term(self.peek()) {
                    self.pos += 1;
                }
            } else {
                return Ok(nl);
            }
        }
    }

