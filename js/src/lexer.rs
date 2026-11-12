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

    fn read_number(&mut self) -> Result<Tok, LexError> {
        let c = self.peek();
        let radix = if c == '0' {
            match self.peek_at(1) {
                'x' | 'X' => 16,
                'o' | 'O' => 8,
                'b' | 'B' => 2,
                _ => 10,
            }
        } else {
            10
        };
        if radix != 10 {
            self.pos += 2;
            let mut digits = String::new();
            while self.peek().is_digit(radix) || self.peek() == '_' {
                if self.peek() != '_' {
                    digits.push(self.peek());
                }
                self.pos += 1;
            }
            if self.peek() == 'n' {
                self.pos += 1;
                return Ok(Tok::BigInt(format!("0{}{digits}", if radix == 16 { 'x' } else if radix == 8 { 'o' } else { 'b' }).into()));
            }
            let mut v: f64 = 0.0;
            for d in digits.chars() {
                v = v * radix as f64 + d.to_digit(radix).unwrap() as f64;
            }
            return self.end_number(Tok::Num(v));
        }
        // octal legado: 0777
        if c == '0' && self.peek_at(1).is_ascii_digit() {
            let start = self.pos;
            self.pos += 1;
            let mut legacy = true;
            while self.peek().is_ascii_digit() {
                if self.peek() >= '8' {
                    legacy = false;
                }
                self.pos += 1;
            }
            let text: String = self.src[start..self.pos].iter().collect();
            let v = if legacy {
                text.chars().skip(1).fold(0.0, |a, d| a * 8.0 + d.to_digit(8).unwrap() as f64)
            } else {
                text.parse().unwrap_or(f64::NAN)
            };
            return self.end_number(Tok::Num(v));
        }
        let mut text = String::new();
        let take_digits = |lx: &mut Lexer, text: &mut String| {
            while lx.peek().is_ascii_digit() || lx.peek() == '_' {
                if lx.peek() != '_' {
                    text.push(lx.peek());
                }
                lx.pos += 1;
            }
        };
        take_digits(self, &mut text);
        if self.peek() == 'n' {
            self.pos += 1;
            return Ok(Tok::BigInt(text.into()));
        }
        if self.peek() == '.' {
            text.push('.');
            self.pos += 1;
            take_digits(self, &mut text);
        }
        if matches!(self.peek(), 'e' | 'E')
            && (self.peek_at(1).is_ascii_digit() || (matches!(self.peek_at(1), '+' | '-') && self.peek_at(2).is_ascii_digit()))
        {
            text.push('e');
            self.pos += 1;
            if matches!(self.peek(), '+' | '-') {
                text.push(self.peek());
                self.pos += 1;
            }
            take_digits(self, &mut text);
        }
        if text.starts_with('.') {
            text.insert(0, '0');
        }
        if text.ends_with('.') {
            text.push('0');
        }
        let v: f64 = text.parse().map_err(|_| self.err("número inválido"))?;
        self.end_number(Tok::Num(v))
    }

    fn end_number(&self, t: Tok) -> Result<Tok, LexError> {
        if is_id_start(self.peek()) {
            return Err(self.err("identificador logo após número"));
        }
        Ok(t)
    }

    /// Escape dentro de string/template. Devolve None para "\" + quebra de linha.
    fn read_escape(&mut self, out: &mut Vec<u16>, in_template: bool) -> Result<bool, LexError> {
        let c = self.peek();
        self.pos += 1;
        let push = |out: &mut Vec<u16>, ch: char| {
            let mut buf = [0u16; 2];
            out.extend_from_slice(ch.encode_utf16(&mut buf));
        };
        match c {
            'n' => out.push(10),
            't' => out.push(9),
            'r' => out.push(13),
            'b' => out.push(8),
            'f' => out.push(12),
            'v' => out.push(11),
            '0' if !self.peek().is_ascii_digit() => out.push(0),
            'x' => {
                let h = self.peek().to_digit(16);
                let l = self.peek_at(1).to_digit(16);
                match (h, l) {
                    (Some(h), Some(l)) => {
                        self.pos += 2;
                        out.push((h * 16 + l) as u16);
                    }
                    _ => return Err(self.err("escape \\x inválido")),
                }
            }
            'u' => {
                // surrogates em escapes separados viram unidades separadas
                if self.peek() == '{' {
                    let ch = self.read_unicode_escape()?;
                    push(out, ch);
                } else {
                    let mut v = 0u32;
                    for _ in 0..4 {
                        let d = self.peek().to_digit(16).ok_or_else(|| self.err("escape \\u inválido"))?;
                        v = v * 16 + d;
                        self.pos += 1;
                    }
                    out.push(v as u16);
                }
            }
            '\r' => {
                if self.peek() == '\n' {
                    self.pos += 1;
                }
                self.line += 1;
            }
            '\n' | '\u{2028}' | '\u{2029}' => self.line += 1,
            '0'..='7' if !in_template => {
                // octal legado
                let mut v = c.to_digit(8).unwrap();
                for _ in 0..2 {
                    match self.peek().to_digit(8) {
                        Some(d) if v * 8 + d < 256 => {
                            v = v * 8 + d;
                            self.pos += 1;
                        }
                        _ => break,
                    }
                }
                out.push(v as u16);
            }
            '\0' if self.pos > self.src.len() => return Err(self.err("string sem fim")),
            other => push(out, other),
        }
        Ok(true)
    }

    fn read_string(&mut self, quote: char) -> Result<Vec<u16>, LexError> {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            if self.pos >= self.src.len() {
                return Err(self.err("string sem fim"));
            }
            let c = self.peek();
            if c == quote {
                self.pos += 1;
                return Ok(out);
            }
            if c == '\\' {
                self.pos += 1;
                self.read_escape(&mut out, false)?;
            } else if c == '\n' || c == '\r' {
                return Err(self.err("quebra de linha dentro de string"));
            } else {
                let mut buf = [0u16; 2];
                out.extend_from_slice(c.encode_utf16(&mut buf));
                self.pos += 1;
            }
        }
    }

