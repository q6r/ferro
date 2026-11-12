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

    pub fn next_token(&mut self) -> Result<Token, LexError> {
        let nl = self.skip_trivia()?;
        let start = self.pos;
        let line = self.line;
        let mut escaped = false;
        let tok = if self.pos >= self.src.len() {
            Tok::Eof
        } else {
            let c = self.peek();
            if is_id_start(c) || c == '\\' {
                let (name, esc) = self.read_ident()?;
                escaped = esc;
                Tok::Ident(name)
            } else if c == '#' && is_id_start(self.peek_at(1)) {
                self.pos += 1;
                Tok::Private(self.read_ident()?.0)
            } else if c.is_ascii_digit() || (c == '.' && self.peek_at(1).is_ascii_digit()) {
                self.read_number()?
            } else if c == '"' || c == '\'' {
                Tok::Str(self.read_string(c)?.into())
            } else if c == '`' {
                self.pos += 1;
                self.read_template_part()?
            } else if c == '/' && self.regex_ok {
                self.read_regex()?
            } else {
                match self.punct() {
                    Some(p) => {
                        self.pos += p.len();
                        Tok::Punct(p)
                    }
                    None => return Err(self.err(format!("caractere inesperado '{c}'"))),
                }
            }
        };
        self.regex_ok = regex_allowed_after(&tok);
        Ok(Token {
            tok,
            start,
            end: self.pos,
            line,
            nl_before: nl,
            escaped,
        })
    }

    /// O operador/pontuação mais longo que começa aqui (mesmo conjunto de PUNCTS).
    fn punct(&self) -> Option<&'static str> {
        let (c1, c2, c3) = (self.peek_at(1), self.peek_at(2), self.peek_at(3));
        let pick = |eq2: &'static str, one: &'static str| if c1 == '=' { eq2 } else { one };
        Some(match self.peek() {
            '{' => "{",
            '}' => "}",
            '(' => "(",
            ')' => ")",
            '[' => "[",
            ']' => "]",
            ';' => ";",
            ',' => ",",
            '~' => "~",
            ':' => ":",
            '@' => "@",
            '.' => {
                if c1 == '.' && c2 == '.' {
                    "..."
                } else {
                    "."
                }
            }
            '?' => match c1 {
                '?' => {
                    if c2 == '=' {
                        "??="
                    } else {
                        "??"
                    }
                }
                // "?." seguido de dígito é "?" e depois ".5"
                '.' if !c2.is_ascii_digit() => "?.",
                _ => "?",
            },
            '=' => match c1 {
                '=' => {
                    if c2 == '=' {
                        "==="
                    } else {
                        "=="
                    }
                }
                '>' => "=>",
                _ => "=",
            },
            '!' => match (c1, c2) {
                ('=', '=') => "!==",
                ('=', _) => "!=",
                _ => "!",
            },
            '+' => match c1 {
                '+' => "++",
                '=' => "+=",
                _ => "+",
            },
            '-' => match c1 {
                '-' => "--",
                '=' => "-=",
                _ => "-",
            },
            '*' => match (c1, c2) {
                ('*', '=') => "**=",
                ('*', _) => "**",
                ('=', _) => "*=",
                _ => "*",
            },
            '/' => pick("/=", "/"),
            '%' => pick("%=", "%"),
            '^' => pick("^=", "^"),
            '<' => match (c1, c2) {
                ('<', '=') => "<<=",
                ('<', _) => "<<",
                ('=', _) => "<=",
                _ => "<",
            },
            '>' => match (c1, c2, c3) {
                ('>', '>', '=') => ">>>=",
                ('>', '>', _) => ">>>",
                ('>', '=', _) => ">>=",
                ('>', _, _) => ">>",
                ('=', _, _) => ">=",
                _ => ">",
            },
            '&' => match (c1, c2) {
                ('&', '=') => "&&=",
                ('&', _) => "&&",
                ('=', _) => "&=",
                _ => "&",
            },
            '|' => match (c1, c2) {
                ('|', '=') => "||=",
                ('|', _) => "||",
                ('=', _) => "|=",
                _ => "|",
            },
            _ => return None,
        })
    }

    fn read_ident(&mut self) -> Result<(Rc<str>, bool), LexError> {
        // caminho rápido: só ASCII, sem escapes
        let start = self.pos;
        let mut end = start;
        while let Some(&c) = self.src.get(end) {
            if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
                end += 1;
            } else {
                break;
            }
        }
        let next = self.src.get(end).copied().unwrap_or('\0');
        if end > start && !self.src[start].is_ascii_digit() && next != '\\' && (next.is_ascii() || !is_id_part(next)) {
            self.buf.clear();
            self.buf.extend(&self.src[start..end]);
            self.pos = end;
            return Ok((self.intern(), false));
        }
        let mut s = String::new();
        let mut escaped = false;
        loop {
            let c = self.peek();
            if c == '\\' && self.peek_at(1) == 'u' {
                self.pos += 2;
                let ch = self.read_unicode_escape()?;
                s.push(ch);
                escaped = true;
            } else if (s.is_empty() && is_id_start(c)) || (!s.is_empty() && is_id_part(c)) {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        if s.is_empty() {
            return Err(self.err("identificador inválido"));
        }
        self.buf = s;
        Ok((self.intern(), escaped))
    }

    /// O Rc<str> do nome em `self.buf` (o mesmo para nomes iguais).
    fn intern(&mut self) -> Rc<str> {
        if let Some(r) = self.names.get(self.buf.as_str()) {
            return r.clone();
        }
        let r: Rc<str> = Rc::from(self.buf.as_str());
        self.names.insert(self.buf.as_str().into(), r.clone());
        r
    }

    /// Depois de "\u": XXXX ou {X...}.
    fn read_unicode_escape(&mut self) -> Result<char, LexError> {
        let code = if self.peek() == '{' {
            self.pos += 1;
            let mut v: u32 = 0;
            let mut n = 0;
            while self.peek() != '}' {
                let d = self.peek().to_digit(16).ok_or_else(|| self.err("escape unicode inválido"))?;
                v = v.checked_mul(16).and_then(|v| v.checked_add(d)).ok_or_else(|| self.err("escape grande demais"))?;
                self.pos += 1;
                n += 1;
            }
            self.pos += 1;
            if n == 0 || v > 0x10FFFF {
                return Err(self.err("escape unicode inválido"));
            }
            v
        } else {
            let mut v = 0;
            for _ in 0..4 {
                let d = self.peek().to_digit(16).ok_or_else(|| self.err("escape unicode inválido"))?;
                v = v * 16 + d;
                self.pos += 1;
            }
            v
        };
        Ok(char::from_u32(code).unwrap_or('\u{FFFD}'))
    }

