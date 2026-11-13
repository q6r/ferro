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

/// Para `new Function(params, body)`.
pub fn parse_function_parts(params: &str, body: &str) -> PResult<Rc<FunctionDef>> {
    parse_function_parts_kind(params, body, "function")
}

/// `prefix`: "function", "async function", "function*" ou "async function*".
pub fn parse_function_parts_kind(params: &str, body: &str, prefix: &str) -> PResult<Rc<FunctionDef>> {
    let src = format!("({prefix} anonymous({params}\n) {{\n{body}\n}})");
    let mut p = Parser::new(&src)?;
    let e = p.parse_expression(false)?;
    match e {
        Expr::Function(f) => Ok(f),
        _ => Err(ParseError {
            msg: "corpo de função inválido".into(),
            line: 1,
            eof: false,
        }),
    }
}

impl Parser {
    fn new(src: &str) -> PResult<Parser> {
        let mut lx = Lexer::new(src);
        let tok = lx.next_token()?;
        Ok(Parser {
            lx,
            tok,
            prev_end: 0,
            prev_line: 1,
            ctx: Ctx::default(),
            strict: false,
            module: false,
        })
    }

    // ------------------------------------------------------------ módulos

    /// Texto de um literal de string (especificador de módulo, nome exportado).
    fn string_literal(&mut self) -> PResult<String> {
        match &self.tok.tok {
            Tok::Str(s) => {
                let t = String::from_utf16_lossy(s);
                self.advance()?;
                Ok(t)
            }
            _ => self.unexpected(),
        }
    }

    /// Nome num `import {}`/`export {}`: identificador (até reservado) ou string.
    fn module_export_name(&mut self) -> PResult<String> {
        if matches!(self.tok.tok, Tok::Str(_)) {
            self.string_literal()
        } else {
            Ok(self.ident_name()?.to_string())
        }
    }

    /// `with { type: "json" }` / `assert {...}` depois do especificador: ignorado.
    fn skip_import_attributes(&mut self) -> PResult<()> {
        if (self.is_kw("with") || (self.is_kw("assert") && !self.tok.nl_before)) && matches!(self.peek()?.tok, Tok::Punct("{")) {
            self.advance()?;
            self.expect("{")?;
            let mut depth = 1;
            while depth > 0 {
                if self.tok.tok == Tok::Eof {
                    return self.unexpected();
                }
                if self.is("{") {
                    depth += 1;
                } else if self.is("}") {
                    depth -= 1;
                }
                self.advance()?;
            }
        }
        Ok(())
    }

    fn from_clause(&mut self) -> PResult<String> {
        self.expect_kw("from")?;
        let spec = self.string_literal()?;
        self.skip_import_attributes()?;
        Ok(spec)
    }

    fn parse_import(&mut self) -> PResult<ImportDecl> {
        self.expect_kw("import")?;
        let mut bindings = Vec::new();
        // import "x";
        if matches!(self.tok.tok, Tok::Str(_)) {
            let specifier = self.string_literal()?;
            self.skip_import_attributes()?;
            self.semicolon()?;
            return Ok(ImportDecl { specifier, bindings });
        }
        let mut need_more = true;
        if let Tok::Ident(_) = &self.tok.tok
            && !self.is("{")
            && !self.is("*")
        {
            let local = self.binding_ident()?;
            bindings.push((ImportName::Default, local));
            need_more = self.eat(",")?;
        }
        if need_more {
            if self.eat("*")? {
                self.expect_kw("as")?;
                let local = self.binding_ident()?;
                bindings.push((ImportName::Namespace, local));
            } else if self.eat("{")? {
                while !self.eat("}")? {
                    let imported = self.module_export_name()?;
                    let local = if self.eat_kw("as")? {
                        self.binding_ident()?
                    } else {
                        Name::from(imported.as_str())
                    };
                    let name = if imported == "default" { ImportName::Default } else { ImportName::Named(imported) };
                    bindings.push((name, local));
                    if !self.is("}") {
                        self.expect(",")?;
                    }
                }
            } else {
                return self.unexpected();
            }
        }
        let specifier = self.from_clause()?;
        self.semicolon()?;
        Ok(ImportDecl { specifier, bindings })
    }

    fn parse_export(&mut self) -> PResult<ExportDecl> {
        self.expect_kw("export")?;
        if self.eat("*")? {
            let alias = if self.eat_kw("as")? { Some(self.module_export_name()?) } else { None };
            let spec = self.from_clause()?;
            self.semicolon()?;
            return Ok(ExportDecl::All(spec, alias));
        }
        if self.eat("{")? {
            let mut names = Vec::new();
            while !self.eat("}")? {
                let local = self.module_export_name()?;
                let exported = if self.eat_kw("as")? { self.module_export_name()? } else { local.clone() };
                names.push((local, exported));
                if !self.is("}") {
                    self.expect(",")?;
                }
            }
            if self.is_kw("from") {
                let spec = self.from_clause()?;
                self.semicolon()?;
                return Ok(ExportDecl::From(spec, names));
            }
            self.semicolon()?;
            return Ok(ExportDecl::Names(names.into_iter().map(|(l, e)| (Name::from(l.as_str()), e)).collect()));
        }
        if self.eat_kw("default")? {
            let is_async_fn = self.is_kw("async")
                && matches!(self.peek()?.tok, Tok::Ident(ref n) if &**n == "function")
                && !self.peek()?.nl_before;
            if self.is_kw("function") || is_async_fn {
                if is_async_fn {
                    self.advance()?;
                }
                let f = self.parse_function(is_async_fn, true)?;
                return Ok(match f.name.clone() {
                    Some(n) => ExportDecl::DefaultDecl(Stmt::Function(f), n),
                    None => ExportDecl::DefaultExpr(Expr::Function(f)),
                });
            }
            if self.is_kw("class") {
                let c = self.parse_class()?;
                return Ok(match c.name.clone() {
                    Some(n) => ExportDecl::DefaultDecl(Stmt::Class(c), n),
                    None => ExportDecl::DefaultExpr(Expr::Class(c)),
                });
            }
            let e = self.parse_assign(false)?;
            self.semicolon()?;
            return Ok(ExportDecl::DefaultExpr(e));
        }
        // export var/let/const/function/class
        let s = self.parse_statement()?;
        match &s {
            Stmt::Var(..) | Stmt::Function(_) | Stmt::Class(_) => Ok(ExportDecl::Decl(s)),
            _ => self.err("export inválido"),
        }
    }

    // ------------------------------------------------------------ tokens

    fn advance(&mut self) -> PResult<Token> {
        let next = self.lx.next_token()?;
        let old = std::mem::replace(&mut self.tok, next);
        self.prev_end = old.end;
        self.prev_line = old.line;
        Ok(old)
    }

    fn line(&self) -> u32 {
        self.tok.line
    }

    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> {
        Err(ParseError {
            msg: msg.into(),
            line: self.tok.line,
            eof: self.tok.tok == Tok::Eof,
        })
    }

    fn unexpected<T>(&self) -> PResult<T> {
        let what = match &self.tok.tok {
            Tok::Eof => "fim do código".to_string(),
            Tok::Ident(n) => format!("'{n}'"),
            Tok::Punct(p) => format!("'{p}'"),
            Tok::Num(n) => format!("número {n}"),
            Tok::Str(_) => "string".to_string(),
            Tok::Template { .. } => "template".to_string(),
            Tok::Regex { .. } => "expressão regular".to_string(),
            Tok::Private(n) => format!("'#{n}'"),
            Tok::BigInt(_) => "BigInt".to_string(),
        };
        self.err(format!("token inesperado: {what}"))
    }

    fn is(&self, p: &str) -> bool {
        matches!(&self.tok.tok, Tok::Punct(x) if *x == p)
    }

    fn is_kw(&self, k: &str) -> bool {
        matches!(&self.tok.tok, Tok::Ident(n) if &**n == k) && !self.tok.escaped
    }

    fn eat(&mut self, p: &str) -> PResult<bool> {
        if self.is(p) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn eat_kw(&mut self, k: &str) -> PResult<bool> {
        if self.is_kw(k) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect(&mut self, p: &str) -> PResult<()> {
        if self.is(p) {
            self.advance()?;
            Ok(())
        } else {
            self.err(format!("esperava '{p}'"))
        }
    }

    fn expect_kw(&mut self, k: &str) -> PResult<()> {
        if self.is_kw(k) {
            self.advance()?;
            Ok(())
        } else {
            self.err(format!("esperava '{k}'"))
        }
    }

    /// Inserção automática de ponto e vírgula.
    fn semicolon(&mut self) -> PResult<()> {
        if self.eat(";")? {
            return Ok(());
        }
        if self.is("}") || self.tok.tok == Tok::Eof || self.tok.nl_before {
            return Ok(());
        }
        self.unexpected()
    }

    /// Estado salvo para retroceder.
    fn save(&self) -> (Token, usize, u32, usize, u32, Ctx) {
        (self.tok.clone(), self.lx.pos(), self.lx.line(), self.prev_end, self.prev_line, self.ctx)
    }

    fn restore(&mut self, s: (Token, usize, u32, usize, u32, Ctx)) {
        let regex_ok = crate::lexer::regex_allowed_after(&s.0.tok);
        self.tok = s.0;
        self.lx.reset(s.1, s.2, regex_ok);
        self.prev_end = s.3;
        self.prev_line = s.4;
        self.ctx = s.5;
    }

    /// Olha o token seguinte sem consumir.
    fn peek(&mut self) -> PResult<Token> {
        let s = self.save();
        self.advance()?;
        let t = self.tok.clone();
        self.restore(s);
        Ok(t)
    }

    fn ident_name(&mut self) -> PResult<Name> {
        match &self.tok.tok {
            Tok::Ident(n) => {
                let n = n.clone();
                self.advance()?;
                Ok(n)
            }
            _ => self.unexpected(),
        }
    }

    /// Identificador usado como nome de variável (não pode ser reservado).
    fn binding_ident(&mut self) -> PResult<Name> {
        match &self.tok.tok {
            Tok::Ident(n) if !RESERVED.contains(&&**n) || self.tok.escaped => {
                if (&**n == "await" && self.ctx.in_async) || (&**n == "yield" && self.ctx.in_generator) {
                    return self.unexpected();
                }
                let n = n.clone();
                self.advance()?;
                Ok(n)
            }
            _ => self.unexpected(),
        }
    }

    // --------------------------------------------------------- instruções

    fn parse_body_until_eof(&mut self) -> PResult<Vec<Stmt>> {
        let mut body = Vec::new();
        self.directives()?;
        while self.tok.tok != Tok::Eof {
            body.push(self.parse_statement()?);
        }
        Ok(body)
    }

    /// "use strict" no começo do programa/função.
    fn directives(&mut self) -> PResult<()> {
        if let Tok::Str(s) = &self.tok.tok {
            let text = String::from_utf16_lossy(s);
            let raw = self.lx.source_slice(self.tok.start + 1, self.tok.end - 1);
            if text == "use strict" && raw == "use strict" {
                let next = self.peek()?;
                if matches!(next.tok, Tok::Punct(";") | Tok::Punct("}") | Tok::Eof) || next.nl_before {
                    self.strict = true;
                }
            }
        }
        Ok(())
    }

    fn parse_block(&mut self) -> PResult<Vec<Stmt>> {
        self.expect("{")?;
        let mut body = Vec::new();
        while !self.is("}") {
            if self.tok.tok == Tok::Eof {
                return self.err("bloco sem '}'");
            }
            body.push(self.parse_statement()?);
        }
        self.advance()?;
        Ok(body)
    }

    fn starts_let_decl(&mut self) -> PResult<bool> {
        if !self.is_kw("let") {
            return Ok(false);
        }
        let next = self.peek()?;
        Ok(match &next.tok {
            Tok::Ident(n) => !matches!(&**n, "in" | "instanceof" | "of") || next.nl_before,
            Tok::Punct("[") | Tok::Punct("{") => true,
            _ => false,
        })
    }

    /// Corpo de if/laço/with: uma instrução, não uma declaração. `annex_b`:
    /// `if (x) function f() {}` vale fora do modo estrito.
    fn parse_body_statement(&mut self, annex_b: bool, labelled_fn_ok: bool) -> PResult<Stmt> {
        // aqui `let` nunca começa declaração (só `let [` é proibido): é identificador
        if self.is_kw("let") && !self.strict && !matches!(self.peek()?.tok, Tok::Punct("[")) {
            let line = self.line();
            let e = self.parse_expression(false)?;
            self.semicolon()?;
            return Ok(Stmt::Expr(e, line));
        }
        let body = self.parse_statement()?;
        if let Some(msg) = Self::decl_as_body(&body, annex_b, self.strict) {
            return self.err(msg);
        }
        if !labelled_fn_ok && Self::is_labelled_function(&body) {
            return self.err("In non-strict mode code, functions can only be declared at top level, inside a block, or as the body of an if statement.");
        }
        Ok(body)
    }

    fn is_labelled_function(s: &Stmt) -> bool {
        match s {
            Stmt::Labeled(_, b) => matches!(**b, Stmt::Function(_)) || Self::is_labelled_function(b),
            _ => false,
        }
    }

    /// Mensagem de erro se a instrução for uma declaração fora de lugar.
    fn decl_as_body(s: &Stmt, fn_ok: bool, strict: bool) -> Option<&'static str> {
        match s {
            Stmt::Var(VarKind::Let | VarKind::Const, ..) | Stmt::Class(_) => {
                Some("Lexical declaration cannot appear in a single-statement context")
            }
            Stmt::Function(f) if !fn_ok || strict || f.is_async || f.is_generator => {
                Some("In strict mode code, functions can only be declared at top level or inside a block.")
            }
            _ => None,
        }
    }

    fn parse_statement(&mut self) -> PResult<Stmt> {
        let line = self.line();
        match self.tok.tok.clone() {
            Tok::Punct("{") => Ok(Stmt::Block(self.parse_block()?)),
            Tok::Punct(";") => {
                self.advance()?;
                Ok(Stmt::Empty)
            }
            Tok::Ident(kw) if !self.tok.escaped => match &*kw {
                "var" | "const" => {
                    self.advance()?;
                    let kind = if &*kw == "var" { VarKind::Var } else { VarKind::Const };
                    let decls = self.parse_var_decls(kind, false)?;
                    self.semicolon()?;
                    Ok(Stmt::Var(kind, decls, line))
                }
                "let" if self.starts_let_decl()? => {
                    self.advance()?;
                    let decls = self.parse_var_decls(VarKind::Let, false)?;
                    self.semicolon()?;
                    Ok(Stmt::Var(VarKind::Let, decls, line))
                }
                "function" => {
                    let f = self.parse_function(false, false)?;
                    Ok(Stmt::Function(f))
                }
                "async" if !self.peek()?.nl_before && matches!(self.peek()?.tok, Tok::Ident(ref n) if &**n == "function") => {
                    self.advance()?;
                    let f = self.parse_function(true, false)?;
                    Ok(Stmt::Function(f))
                }
                "class" => Ok(Stmt::Class(self.parse_class()?)),
                "if" => {
                    self.advance()?;
                    self.expect("(")?;
                    let test = self.parse_expression(false)?;
                    self.expect(")")?;
                    let cons = self.parse_body_statement(true, false)?;
                    let alt = if self.eat_kw("else")? { Some(Box::new(self.parse_body_statement(true, false)?)) } else { None };
                    Ok(Stmt::If(test, Box::new(cons), alt))
                }
                "for" => self.parse_for(),
                "while" => {
                    self.advance()?;
                    self.expect("(")?;
                    let test = self.parse_expression(false)?;
                    self.expect(")")?;
                    let body = self.parse_body_statement(false, false)?;
                    Ok(Stmt::While(test, Box::new(body)))
                }
                "do" => {
                    self.advance()?;
                    let body = self.parse_body_statement(false, false)?;
                    self.expect_kw("while")?;
                    self.expect("(")?;
                    let test = self.parse_expression(false)?;
                    self.expect(")")?;
                    self.eat(";")?;
                    Ok(Stmt::DoWhile(Box::new(body), test))
                }
                "return" => {
                    self.advance()?;
                    let arg = if self.is(";") || self.is("}") || self.tok.tok == Tok::Eof || self.tok.nl_before {
                        None
                    } else {
                        Some(self.parse_expression(false)?)
                    };
                    self.semicolon()?;
                    Ok(Stmt::Return(arg, line))
                }
                "break" | "continue" => {
                    self.advance()?;
                    let label = match &self.tok.tok {
                        Tok::Ident(n) if !self.tok.nl_before && !RESERVED.contains(&&**n) => {
                            let n = n.clone();
                            self.advance()?;
                            Some(n)
                        }
                        _ => None,
                    };
                    self.semicolon()?;
                    Ok(if &*kw == "break" { Stmt::Break(label, line) } else { Stmt::Continue(label, line) })
                }
                "throw" => {
                    self.advance()?;
                    if self.tok.nl_before {
                        return self.err("quebra de linha depois de throw");
                    }
                    let e = self.parse_expression(false)?;
                    self.semicolon()?;
                    Ok(Stmt::Throw(e, line))
                }
                "try" => self.parse_try(),
                "switch" => self.parse_switch(),
                "debugger" => {
                    self.advance()?;
                    self.semicolon()?;
                    Ok(Stmt::Debugger)
                }
                "import" if !matches!(self.peek()?.tok, Tok::Punct("(") | Tok::Punct(".")) => {
                    self.err("módulos (import/export) ainda não são suportados")
                }
                "export" => self.err("módulos (import/export) ainda não são suportados"),
                "with" => {
                    if self.strict {
                        return self.err("Strict mode code may not include a with statement");
                    }
                    self.advance()?;
                    self.expect("(")?;
                    let obj = self.parse_expression(false)?;
                    self.expect(")")?;
                    let body = self.parse_body_statement(false, false)?;
                    Ok(Stmt::With(obj, Box::new(body)))
                }
                _ => {
                    // rótulo?
                    if !RESERVED.contains(&&*kw) && matches!(self.peek()?.tok, Tok::Punct(":")) {
                        self.advance()?;
                        self.advance()?;
                        let body = self.parse_body_statement(true, true)?;
                        return Ok(Stmt::Labeled(kw, Box::new(body)));
                    }
                    let e = self.parse_expression(false)?;
                    self.semicolon()?;
                    Ok(Stmt::Expr(e, line))
                }
            },
            _ => {
                let e = self.parse_expression(false)?;
                self.semicolon()?;
                Ok(Stmt::Expr(e, line))
            }
        }
    }

    fn parse_var_decls(&mut self, _kind: VarKind, no_in: bool) -> PResult<Vec<VarDecl>> {
        let mut decls = Vec::new();
        loop {
            let target = self.parse_binding_target()?;
            let init = if self.eat("=")? { Some(self.parse_assign(no_in)?) } else { None };
            decls.push(VarDecl { target, init });
            if !self.eat(",")? {
                break;
            }
        }
        Ok(decls)
    }

    fn parse_for(&mut self) -> PResult<Stmt> {
        self.advance()?;
        let is_await = self.eat_kw("await")?;
        self.expect("(")?;
        let line = self.line();
        let mut init: Option<ForInit> = None;
        if self.eat(";")? {
            // sem inicialização
        } else {
            let decl_kind = if self.is_kw("var") {
                Some(VarKind::Var)
            } else if self.is_kw("const") {
                Some(VarKind::Const)
            } else if self.starts_let_decl()? {
                Some(VarKind::Let)
            } else {
                None
            };
            if let Some(kind) = decl_kind {
                self.advance()?;
                let target = self.parse_binding_target()?;
                if self.is_kw("of") || self.is_kw("in") {
                    let of = self.is_kw("of");
                    self.advance()?;
                    let right = if of { self.parse_assign(false)? } else { self.parse_expression(false)? };
                    self.expect(")")?;
                    let body = Box::new(self.parse_body_statement(false, false)?);
                    let left = ForHead::Var(kind, target);
                    return Ok(if of {
                        Stmt::ForOf { left, right, body, is_await }
                    } else {
                        Stmt::ForIn { left, right, body }
                    });
                }
                let first_init = if self.eat("=")? { Some(self.parse_assign(true)?) } else { None };
                let mut decls = vec![VarDecl { target, init: first_init }];
                if self.eat(",")? {
                    decls.extend(self.parse_var_decls(kind, true)?);
                }
                init = Some(ForInit::Var(kind, decls));
            } else {
                let e = self.parse_expression(true)?;
                if self.is_kw("of") || self.is_kw("in") {
                    let of = self.is_kw("of");
                    self.advance()?;
                    let right = if of { self.parse_assign(false)? } else { self.parse_expression(false)? };
                    self.expect(")")?;
                    let body = Box::new(self.parse_body_statement(false, false)?);
                    let left = ForHead::Pattern(self.expr_to_pattern(e)?);
                    return Ok(if of {
                        Stmt::ForOf { left, right, body, is_await }
                    } else {
                        Stmt::ForIn { left, right, body }
                    });
                }
                init = Some(ForInit::Expr(e));
            }
            self.expect(";")?;
        }
        let _ = line;
        let test = if self.is(";") { None } else { Some(self.parse_expression(false)?) };
        self.expect(";")?;
        let update = if self.is(")") { None } else { Some(self.parse_expression(false)?) };
        self.expect(")")?;
        let body = Box::new(self.parse_body_statement(false, false)?);
        Ok(Stmt::For { init, test, update, body })
    }

    fn parse_try(&mut self) -> PResult<Stmt> {
        self.advance()?;
        let block = self.parse_block()?;
        let mut param = None;
        let mut handler = None;
        if self.eat_kw("catch")? {
            if self.eat("(")? {
                param = Some(self.parse_binding_target()?);
                self.expect(")")?;
            }
            handler = Some(self.parse_block()?);
        }
        let finalizer = if self.eat_kw("finally")? { Some(self.parse_block()?) } else { None };
        if handler.is_none() && finalizer.is_none() {
            return self.err("try sem catch nem finally");
        }
        Ok(Stmt::Try {
            block,
            param,
            handler,
            finalizer,
        })
    }

    fn parse_switch(&mut self) -> PResult<Stmt> {
        self.advance()?;
        self.expect("(")?;
        let disc = self.parse_expression(false)?;
        self.expect(")")?;
        self.expect("{")?;
        let mut cases = Vec::new();
        while !self.eat("}")? {
            let test = if self.eat_kw("default")? {
                None
            } else {
                self.expect_kw("case")?;
                Some(self.parse_expression(false)?)
            };
            self.expect(":")?;
            let mut body = Vec::new();
            while !self.is("}") && !self.is_kw("case") && !self.is_kw("default") {
                if self.tok.tok == Tok::Eof {
                    return self.err("switch sem '}'");
                }
                body.push(self.parse_statement()?);
            }
            cases.push(Case { test, body });
        }
        Ok(Stmt::Switch(disc, cases))
    }

    // ----------------------------------------------------------- padrões

    fn parse_binding_target(&mut self) -> PResult<Pattern> {
        if self.is("[") {
            self.advance()?;
            let mut elems = Vec::new();
            let mut rest = None;
            loop {
                if self.eat("]")? {
                    break;
                }
                if self.is(",") {
                    self.advance()?;
                    elems.push(None);
                    continue;
                }
                if self.eat("...")? {
                    rest = Some(Box::new(self.parse_binding_target()?));
                    self.eat(",")?;
                    self.expect("]")?;
                    break;
                }
                elems.push(Some(self.parse_binding_element()?));
                if !self.is("]") {
                    self.expect(",")?;
                }
            }
            return Ok(Pattern::Array { elems, rest });
        }
        if self.is("{") {
            self.advance()?;
            let mut props = Vec::new();
            let mut rest = None;
            while !self.eat("}")? {
                if self.eat("...")? {
                    rest = Some(Box::new(Pattern::Ident(self.binding_ident()?)));
                    self.eat(",")?;
                    self.expect("}")?;
                    break;
                }
                let key = self.parse_prop_key()?;
                let value = if self.eat(":")? {
                    self.parse_binding_element()?
                } else {
                    let name = match &key {
                        PropKeyExpr::Name(n) => n.clone(),
                        _ => return self.unexpected(),
                    };
                    let mut p = Pattern::Ident(name);
                    if self.eat("=")? {
                        p = Pattern::Default(Box::new(p), Box::new(self.parse_assign(false)?));
                    }
                    p
                };
                props.push((key, value));
                if !self.is("}") {
                    self.expect(",")?;
                }
            }
            return Ok(Pattern::Object { props, rest });
        }
        Ok(Pattern::Ident(self.binding_ident()?))
    }

    fn parse_binding_element(&mut self) -> PResult<Pattern> {
        let target = self.parse_binding_target()?;
        if self.eat("=")? {
            return Ok(Pattern::Default(Box::new(target), Box::new(self.parse_assign(false)?)));
        }
        Ok(target)
    }

    /// Converte uma expressão (literal de objeto/array, identificador, membro)
    /// em padrão de atribuição: `[a, b] = ...`, `({x} = o)`.
    fn expr_to_pattern(&self, e: Expr) -> PResult<Pattern> {
        Ok(match e {
            Expr::Ident(n, _) => Pattern::Ident(n),
            Expr::Member { .. } | Expr::Index { .. } | Expr::PrivateMember(..) | Expr::SuperMember(_) | Expr::SuperIndex(_) => {
                Pattern::Target(Box::new(e))
            }
            Expr::Assign(AssignOp::Assign, target, value) => Pattern::Default(target, value),
            Expr::Array(elems) => {
                let mut out = Vec::new();
                let mut rest = None;
                let n = elems.len();
                for (i, el) in elems.into_iter().enumerate() {
                    match el {
                        ArrayElem::Hole => out.push(None),
                        ArrayElem::Expr(x) => out.push(Some(self.expr_to_pattern(x)?)),
                        ArrayElem::Spread(x) if i == n - 1 => rest = Some(Box::new(self.expr_to_pattern(x)?)),
                        ArrayElem::Spread(_) => return self.err("rest precisa ser o último elemento"),
                    }
                }
                Pattern::Array { elems: out, rest }
            }
            Expr::Object(props) => {
                let mut out = Vec::new();
                let mut rest = None;
                for p in props {
                    match p.kind {
                        PropKind::Spread => rest = Some(Box::new(self.expr_to_pattern(p.value)?)),
                        PropKind::Init => out.push((p.key, self.expr_to_pattern(p.value)?)),
                        _ => return self.err("padrão de desestruturação inválido"),
                    }
                }
                Pattern::Object { props: out, rest }
            }
            _ => return self.err("alvo de atribuição inválido"),
        })
    }

    // ---------------------------------------------------------- expressões

    pub fn parse_expression(&mut self, no_in: bool) -> PResult<Expr> {
        let first = self.parse_assign(no_in)?;
        if !self.is(",") {
            return Ok(first);
        }
        let mut list = vec![first];
        while self.eat(",")? {
            list.push(self.parse_assign(no_in)?);
        }
        Ok(Expr::Seq(list))
    }

    fn parse_assign(&mut self, no_in: bool) -> PResult<Expr> {
        // arrow functions e async arrow
        if let Some(arrow) = self.try_arrow(no_in)? {
            return Ok(arrow);
        }
        if self.is_kw("yield") && self.ctx.in_generator {
            self.advance()?;
            if self.tok.nl_before || matches!(self.tok.tok, Tok::Punct(")" | "]" | "}" | "," | ";" | ":")) || self.tok.tok == Tok::Eof {
                return Ok(Expr::Yield(None, false));
            }
            let delegate = self.eat("*")?;
            let arg = self.parse_assign(no_in)?;
            return Ok(Expr::Yield(Some(Box::new(arg)), delegate));
        }
        let start_line = self.line();
        let left = self.parse_conditional(no_in)?;
        if let Tok::Punct(p) = &self.tok.tok
            && let Some(op) = assign_op(p)
        {
            self.advance()?;
            let target = if op == AssignOp::Assign {
                self.expr_to_pattern(left)?
            } else {
                match left {
                    Expr::Ident(..) | Expr::Member { .. } | Expr::Index { .. } | Expr::PrivateMember(..) | Expr::SuperMember(_) | Expr::SuperIndex(_) => {
                        if let Expr::Ident(n, _) = left {
                            Pattern::Ident(n)
                        } else {
                            Pattern::Target(Box::new(left))
                        }
                    }
                    _ => {
                        return Err(ParseError {
                            msg: "alvo de atribuição inválido".into(),
                            line: start_line,
                            eof: false,
                        });
                    }
                }
            };
            let value = self.parse_assign(no_in)?;
            return Ok(Expr::Assign(op, Box::new(target), Box::new(value)));
        }
        Ok(left)
    }

    /// Tenta ler uma arrow function; se não for, devolve None sem consumir nada.
    fn try_arrow(&mut self, no_in: bool) -> PResult<Option<Expr>> {
        let start = self.tok.start;
        let line = self.line();
        let is_async = self.is_kw("async") && {
            let n = self.peek()?;
            !n.nl_before && matches!(n.tok, Tok::Punct("(") | Tok::Ident(_))
        };
        // x => ...
        if let Tok::Ident(_) = &self.tok.tok
            && !is_async
        {
            let next = self.peek()?;
            if next.tok == Tok::Punct("=>") && !next.nl_before {
                let name = self.binding_ident()?;
                self.advance()?;
                let f = self.arrow_body(vec![Pattern::Ident(name)], None, false, start, line, no_in)?;
                return Ok(Some(f));
            }
            return Ok(None);
        }
        if !self.is("(") && !is_async {
            return Ok(None);
        }
        let saved = self.save();
        if is_async {
            self.advance()?;
            // async x => ...
            if let Tok::Ident(_) = &self.tok.tok {
                let next = self.peek()?;
                if next.tok == Tok::Punct("=>") && !next.nl_before {
                    let name = self.binding_ident()?;
                    self.advance()?;
                    let f = self.arrow_body(vec![Pattern::Ident(name)], None, true, start, line, no_in)?;
                    return Ok(Some(f));
                }
                self.restore(saved);
                return Ok(None);
            }
        }
        // ( params ) =>
        let attempt = (|| -> PResult<Option<(Vec<Pattern>, Option<Pattern>)>> {
            self.expect("(")?;
            let mut params = Vec::new();
            let mut rest = None;
            while !self.is(")") {
                if self.eat("...")? {
                    rest = Some(self.parse_binding_target()?);
                    break;
                }
                params.push(self.parse_binding_element()?);
                if !self.is(")") {
                    self.expect(",")?;
                }
            }
            self.expect(")")?;
            if self.is("=>") && !self.tok.nl_before {
                Ok(Some((params, rest)))
            } else {
                Ok(None)
            }
        })();
        match attempt {
            Ok(Some((params, rest))) => {
                self.advance()?;
                let f = self.arrow_body(params, rest, is_async, start, line, no_in)?;
                Ok(Some(f))
            }
            _ => {
                self.restore(saved);
                Ok(None)
            }
        }
    }

    fn arrow_body(
        &mut self,
        params: Vec<Pattern>,
        rest: Option<Pattern>,
        is_async: bool,
        start: usize,
        line: u32,
        no_in: bool,
    ) -> PResult<Expr> {
        let saved_ctx = self.ctx;
        self.ctx.in_async = is_async;
        self.ctx.in_generator = false;
        self.ctx.in_function = true;
        let mut strict = self.strict;
        let body = if self.is("{") {
            let saved_strict = self.strict;
            self.advance()?;
            self.directives()?;
            let mut stmts = Vec::new();
            while !self.eat("}")? {
                if self.tok.tok == Tok::Eof {
                    return self.err("função sem '}'");
                }
                stmts.push(self.parse_statement()?);
            }
            strict = self.strict;
            self.strict = saved_strict;
            FnBody::Block(stmts)
        } else {
            FnBody::Expr(Box::new(self.parse_assign(no_in)?))
        };
        self.ctx = saved_ctx;
        let source = self.lx.source_slice(start, self.prev_end);
        Ok(Expr::Function(Rc::new(FunctionDef {
            name: None,
            params,
            rest,
            body,
            kind: FnKind::Arrow,
            is_async,
            is_generator: false,
            strict,
            source: source.into(),
            line,
        })))
    }

    fn parse_conditional(&mut self, no_in: bool) -> PResult<Expr> {
        let test = self.parse_binary(0, no_in)?;
        if !self.eat("?")? {
            return Ok(test);
        }
        let cons = self.parse_assign(false)?;
        self.expect(":")?;
        let alt = self.parse_assign(no_in)?;
        Ok(Expr::Cond(Box::new(test), Box::new(cons), Box::new(alt)))
    }

    fn parse_binary(&mut self, min_prec: u8, no_in: bool) -> PResult<Expr> {
        let mut left = if let Tok::Private(name) = self.tok.tok.clone() {
            // #x in obj
            self.advance()?;
            if !self.is_kw("in") {
                return self.unexpected();
            }
            self.advance()?;
            let right = self.parse_binary(9, no_in)?;
            Expr::PrivateIn(name, Box::new(right))
        } else {
            self.parse_unary()?
        };
        loop {
            let Some((prec, op)) = bin_prec(&self.tok.tok, no_in) else { break };
            if prec <= min_prec && !(prec == 12 && min_prec == 12) {
                break;
            }
            if prec < min_prec || (prec == min_prec && prec != 12) {
                break;
            }
            self.advance()?;
            // ** é associativo à direita
            let right = if prec == 12 { self.parse_binary(11, no_in)? } else { self.parse_binary(prec, no_in)? };
            left = match op {
                BinOrLog::Bin(b) => Expr::Binary(b, Box::new(left), Box::new(right)),
                BinOrLog::Log(l) => Expr::Logical(l, Box::new(left), Box::new(right)),
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let op = match &self.tok.tok {
            Tok::Punct("!") => Some(UnOp::Not),
            Tok::Punct("-") => Some(UnOp::Neg),
            Tok::Punct("+") => Some(UnOp::Plus),
            Tok::Punct("~") => Some(UnOp::BitNot),
            Tok::Ident(n) if !self.tok.escaped && &**n == "typeof" => Some(UnOp::Typeof),
            Tok::Ident(n) if !self.tok.escaped && &**n == "void" => Some(UnOp::Void),
            Tok::Ident(n) if !self.tok.escaped && &**n == "delete" => Some(UnOp::Delete),
            _ => None,
        };
        if let Some(op) = op {
            self.advance()?;
            let arg = self.parse_unary()?;
            return Ok(Expr::Unary(op, Box::new(arg)));
        }
        if self.is_kw("await") && (self.ctx.in_async || !self.ctx.in_function) && self.ctx.in_async {
            self.advance()?;
            let arg = self.parse_unary()?;
            return Ok(Expr::Await(Box::new(arg)));
        }
        if self.is("++") || self.is("--") {
            let inc = self.is("++");
            self.advance()?;
            let target = self.parse_unary()?;
            return Ok(Expr::Update {
                inc,
                prefix: true,
                target: Box::new(target),
            });
        }
        let e = self.parse_lhs()?;
        if (self.is("++") || self.is("--")) && !self.tok.nl_before {
            let inc = self.is("++");
            self.advance()?;
            return Ok(Expr::Update {
                inc,
                prefix: false,
                target: Box::new(e),
            });
        }
        Ok(e)
    }

    fn parse_args(&mut self) -> PResult<Vec<ArrayElem>> {
        self.expect("(")?;
        let mut args = Vec::new();
        while !self.is(")") {
            if self.eat("...")? {
                args.push(ArrayElem::Spread(self.parse_assign(false)?));
            } else {
                args.push(ArrayElem::Expr(self.parse_assign(false)?));
            }
            if !self.is(")") {
                self.expect(",")?;
            }
        }
        self.advance()?;
        Ok(args)
    }

    fn parse_lhs(&mut self) -> PResult<Expr> {
        let mut e = if self.is_kw("new") {
            self.parse_new()?
        } else if self.is_kw("super") {
            self.advance()?;
            if self.is("(") {
                Expr::SuperCall(self.parse_args()?)
            } else if self.eat(".")? {
                Expr::SuperMember(self.ident_name()?)
            } else if self.eat("[")? {
                let idx = self.parse_expression(false)?;
                self.expect("]")?;
                Expr::SuperIndex(Box::new(idx))
            } else {
                return self.unexpected();
            }
        } else if self.is_kw("import") {
            self.advance()?;
            if self.eat(".")? {
                self.ident_name()?;
                if !self.module {
                    return self.err("import.meta só pode ser usado em módulos");
                }
                Expr::ImportMeta
            } else {
                let args = self.parse_args()?;
                // import(especificador, opções?): no máximo 2, sem `...`
                if args.len() > 2 || args.iter().any(|a| !matches!(a, ArrayElem::Expr(_))) {
                    return self.err("import() aceita no máximo 2 argumentos, sem '...'");
                }
                match args.into_iter().next() {
                    Some(ArrayElem::Expr(e)) => Expr::ImportCall(Box::new(e)),
                    _ => return self.err("import() precisa de um especificador"),
                }
            }
        } else {
            self.parse_primary()?
        };
        let mut optional_seen = false;
        loop {
            let line = self.line();
            if self.eat(".")? {
                if let Tok::Private(n) = self.tok.tok.clone() {
                    self.advance()?;
                    e = Expr::PrivateMember(Box::new(e), n);
                } else {
                    let prop = self.ident_name()?;
                    e = Expr::Member {
                        obj: Box::new(e),
                        prop,
                        optional: false,
                    };
                }
            } else if self.eat("?.")? {
                optional_seen = true;
                if self.is("(") {
                    let args = self.parse_args()?;
                    e = Expr::Call {
                        callee: Box::new(e),
                        args,
                        optional: true,
                        line,
                    };
                } else if self.eat("[")? {
                    let idx = self.parse_expression(false)?;
                    self.expect("]")?;
                    e = Expr::Index {
                        obj: Box::new(e),
                        index: Box::new(idx),
                        optional: true,
                    };
                } else if let Tok::Private(n) = self.tok.tok.clone() {
                    self.advance()?;
                    e = Expr::PrivateMember(Box::new(e), n);
                } else {
                    let prop = self.ident_name()?;
                    e = Expr::Member {
                        obj: Box::new(e),
                        prop,
                        optional: true,
                    };
                }
            } else if self.eat("[")? {
                let idx = self.parse_expression(false)?;
                self.expect("]")?;
                e = Expr::Index {
                    obj: Box::new(e),
                    index: Box::new(idx),
                    optional: false,
                };
            } else if self.is("(") {
                let args = self.parse_args()?;
                e = Expr::Call {
                    callee: Box::new(e),
                    args,
                    optional: false,
                    line,
                };
            } else if let Tok::Template { .. } = &self.tok.tok {
                if optional_seen {
                    return self.err("template marcado em cadeia opcional");
                }
                let (cooked, raw, exprs) = self.parse_template_parts()?;
                e = Expr::Tagged {
                    tag: Box::new(e),
                    cooked,
                    raw,
                    exprs,
                };
            } else {
                break;
            }
        }
        if optional_seen {
            e = Expr::OptionalChain(Box::new(e));
        }
        Ok(e)
    }

    fn parse_new(&mut self) -> PResult<Expr> {
        let line = self.line();
        self.expect_kw("new")?;
        if self.eat(".")? {
            let n = self.ident_name()?;
            if &*n != "target" {
                return self.err("esperava new.target");
            }
            return Ok(Expr::NewTarget);
        }
        let mut callee = if self.is_kw("new") { self.parse_new()? } else { self.parse_primary()? };
        loop {
            if self.eat(".")? {
                if let Tok::Private(n) = self.tok.tok.clone() {
                    self.advance()?;
                    callee = Expr::PrivateMember(Box::new(callee), n);
                } else {
                    let prop = self.ident_name()?;
                    callee = Expr::Member {
                        obj: Box::new(callee),
                        prop,
                        optional: false,
                    };
                }
            } else if self.eat("[")? {
                let idx = self.parse_expression(false)?;
                self.expect("]")?;
                callee = Expr::Index {
                    obj: Box::new(callee),
                    index: Box::new(idx),
                    optional: false,
                };
            } else {
                break;
            }
        }
        let args = if self.is("(") { self.parse_args()? } else { Vec::new() };
        Ok(Expr::New(Box::new(callee), args, line))
    }

    fn parse_template_parts(&mut self) -> PResult<(Vec<Option<Rc<[u16]>>>, Vec<Rc<str>>, Vec<Expr>)> {
        let mut cooked = Vec::new();
        let mut raw = Vec::new();
        let mut exprs = Vec::new();
        loop {
            let (c, r, tail) = match &self.tok.tok {
                Tok::Template { cooked, raw, tail } => (cooked.clone(), raw.clone(), *tail),
                _ => return self.unexpected(),
            };
            cooked.push(c);
            raw.push(r);
            if tail {
                self.advance()?;
                break;
            }
            self.advance()?;
            exprs.push(self.parse_expression(false)?);
            if !self.is("}") {
                return self.err("esperava '}' no template");
            }
            // o lexer continua o template logo depois do "}"
            let tpl = self.lx.read_template_part()?;
            let line = self.lx.line();
            self.tok = Token {
                tok: tpl,
                start: self.tok.start,
                end: self.lx.pos(),
                line,
                nl_before: false,
                escaped: false,
            };
        }
        Ok((cooked, raw, exprs))
    }

    fn parse_prop_key(&mut self) -> PResult<PropKeyExpr> {
        Ok(match self.tok.tok.clone() {
            Tok::Ident(n) => {
                self.advance()?;
                PropKeyExpr::Name(n)
            }
            Tok::Str(s) => {
                self.advance()?;
                PropKeyExpr::Name(String::from_utf16_lossy(&s).into())
            }
            Tok::Num(n) => {
                self.advance()?;
                PropKeyExpr::Name(crate::conv::number_to_string(n).into())
            }
            Tok::BigInt(b) => {
                self.advance()?;
                PropKeyExpr::Name(b)
            }
            Tok::Private(n) => {
                self.advance()?;
                PropKeyExpr::Private(n)
            }
            Tok::Punct("[") => {
                self.advance()?;
                let e = self.parse_assign(false)?;
                self.expect("]")?;
                PropKeyExpr::Computed(Box::new(e))
            }
            _ => return self.unexpected(),
        })
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let line = self.line();
        match self.tok.tok.clone() {
            Tok::Num(n) => {
                self.advance()?;
                Ok(Expr::Num(n))
            }
            Tok::BigInt(b) => {
                self.advance()?;
                Ok(Expr::BigInt(b))
            }
            Tok::Str(s) => {
                self.advance()?;
                Ok(Expr::Str(s))
            }
            Tok::Template { .. } => {
                let (cooked, _, exprs) = self.parse_template_parts()?;
                let mut parts = Vec::new();
                for c in cooked {
                    match c {
                        Some(c) => parts.push(c),
                        None => return self.err("escape inválido em template"),
                    }
                }
                Ok(Expr::Template(parts, exprs))
            }
            Tok::Regex { pattern, flags } => {
                self.advance()?;
                Ok(Expr::Regex(pattern, flags))
            }
            Tok::Punct("(") => {
                self.advance()?;
                let e = self.parse_expression(false)?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Punct("[") => {
                self.advance()?;
                let mut elems = Vec::new();
                while !self.eat("]")? {
                    if self.is(",") {
                        self.advance()?;
                        elems.push(ArrayElem::Hole);
                        continue;
                    }
                    if self.eat("...")? {
                        elems.push(ArrayElem::Spread(self.parse_assign(false)?));
                    } else {
                        elems.push(ArrayElem::Expr(self.parse_assign(false)?));
                    }
                    if !self.is("]") {
                        self.expect(",")?;
                    }
                }
                Ok(Expr::Array(elems))
            }
            Tok::Punct("{") => self.parse_object(),
            Tok::Ident(n) if !self.tok.escaped => match &*n {
                "this" => {
                    self.advance()?;
                    Ok(Expr::This)
                }
                "null" => {
                    self.advance()?;
                    Ok(Expr::Null)
                }
                "true" => {
                    self.advance()?;
                    Ok(Expr::Bool(true))
                }
                "false" => {
                    self.advance()?;
                    Ok(Expr::Bool(false))
                }
                "function" => Ok(Expr::Function(self.parse_function(false, true)?)),
                "async" if matches!(self.peek()?.tok, Tok::Ident(ref k) if &**k == "function") && !self.peek()?.nl_before => {
                    self.advance()?;
                    Ok(Expr::Function(self.parse_function(true, true)?))
                }
                "class" => Ok(Expr::Class(self.parse_class()?)),
                _ if RESERVED.contains(&&*n) => self.unexpected(),
                _ => {
                    self.advance()?;
                    Ok(Expr::Ident(n, line))
                }
            },
            Tok::Ident(n) => {
                self.advance()?;
                Ok(Expr::Ident(n, line))
            }
            _ => self.unexpected(),
        }
    }

    fn parse_object(&mut self) -> PResult<Expr> {
        self.expect("{")?;
        let mut props = Vec::new();
        while !self.eat("}")? {
            if self.eat("...")? {
                props.push(Prop {
                    key: PropKeyExpr::Name("".into()),
                    value: self.parse_assign(false)?,
                    kind: PropKind::Spread,
                });
                if !self.is("}") {
                    self.expect(",")?;
                }
                continue;
            }
            let start = self.tok.start;
            let line = self.line();
            // prefixos get/set/async/*
            let mut is_async = false;
            let mut is_gen = false;
            let mut accessor: Option<PropKind> = None;
            if let Tok::Ident(n) = self.tok.tok.clone()
                && matches!(&*n, "get" | "set" | "async")
            {
                let next = self.peek()?;
                let is_prefix = !matches!(next.tok, Tok::Punct(",") | Tok::Punct(":") | Tok::Punct("(") | Tok::Punct("}") | Tok::Punct("="));
                if is_prefix {
                    match &*n {
                        "get" => accessor = Some(PropKind::Get),
                        "set" => accessor = Some(PropKind::Set),
                        _ => is_async = true,
                    }
                    self.advance()?;
                }
            }
            if self.eat("*")? {
                is_gen = true;
            }
            let key = self.parse_prop_key()?;
            if let Some(kind) = accessor {
                let fkind = if matches!(kind, PropKind::Get) { FnKind::Getter } else { FnKind::Setter };
                let f = self.parse_function_rest(None, fkind, false, false, start, line)?;
                props.push(Prop {
                    key,
                    value: Expr::Function(f),
                    kind,
                });
            } else if self.is("(") {
                let name = match &key {
                    PropKeyExpr::Name(n) => Some(n.clone()),
                    _ => None,
                };
                let f = self.parse_function_rest(name, FnKind::Method, is_async, is_gen, start, line)?;
                props.push(Prop {
                    key,
                    value: Expr::Function(f),
                    kind: PropKind::Init,
                });
            } else if self.eat(":")? {
                let value = self.parse_assign(false)?;
                props.push(Prop {
                    key,
                    value,
                    kind: PropKind::Init,
                });
            } else {
                // abreviação { x } ou { x = 1 } (só vale em desestruturação)
                let name = match &key {
                    PropKeyExpr::Name(n) => n.clone(),
                    _ => return self.unexpected(),
                };
                let mut value = Expr::Ident(name.clone(), line);
                if self.eat("=")? {
                    let def = self.parse_assign(false)?;
                    value = Expr::Assign(AssignOp::Assign, Box::new(Pattern::Ident(name)), Box::new(def));
                }
                props.push(Prop {
                    key,
                    value,
                    kind: PropKind::Init,
                });
            }
            if !self.is("}") {
                self.expect(",")?;
            }
        }
        Ok(Expr::Object(props))
    }

    // ------------------------------------------------------------- funções

    fn parse_function(&mut self, is_async: bool, is_expr: bool) -> PResult<Rc<FunctionDef>> {
        let start = self.tok.start;
        let line = self.line();
        self.expect_kw("function")?;
        let is_gen = self.eat("*")?;
        let name = if let Tok::Ident(_) = &self.tok.tok {
            Some(self.binding_ident()?)
        } else if is_expr {
            None
        } else {
            return self.err("função sem nome");
        };
        self.parse_function_rest(name, FnKind::Normal, is_async, is_gen, start, line)
    }

    /// Parâmetros e corpo (a partir do "(").
    fn parse_function_rest(
        &mut self,
        name: Option<Name>,
        kind: FnKind,
        is_async: bool,
        is_generator: bool,
        start: usize,
        line: u32,
    ) -> PResult<Rc<FunctionDef>> {
        let saved_ctx = self.ctx;
        let saved_strict = self.strict;
        self.ctx.in_function = true;
        self.ctx.in_async = is_async;
        self.ctx.in_generator = is_generator;
        self.expect("(")?;
        let mut params = Vec::new();
        let mut rest = None;
        while !self.is(")") {
            if self.eat("...")? {
                rest = Some(self.parse_binding_target()?);
                break;
            }
            params.push(self.parse_binding_element()?);
            if !self.is(")") {
                self.expect(",")?;
            }
        }
        self.expect(")")?;
        self.expect("{")?;
        self.directives()?;
        let mut body = Vec::new();
        while !self.eat("}")? {
            if self.tok.tok == Tok::Eof {
                return self.err("função sem '}'");
            }
            body.push(self.parse_statement()?);
        }
        let strict = self.strict;
        self.ctx = saved_ctx;
        self.strict = saved_strict;
        let source = self.lx.source_slice(start, self.prev_end);
        Ok(Rc::new(FunctionDef {
            name,
            params,
            rest,
            body: FnBody::Block(body),
            kind,
            is_async,
            is_generator,
            strict,
            source: source.into(),
            line,
        }))
    }

    fn parse_class(&mut self) -> PResult<Rc<ClassDef>> {
        let start = self.tok.start;
        let line = self.line();
        self.expect_kw("class")?;
        let name = match &self.tok.tok {
            Tok::Ident(n) if &**n != "extends" => Some(self.binding_ident()?),
            _ => None,
        };
        let saved_strict = self.strict;
        self.strict = true;
        let superclass = if self.eat_kw("extends")? { Some(self.parse_lhs()?) } else { None };
        let saved_ctx = self.ctx;
        self.ctx.in_class = true;
        self.expect("{")?;
        let mut constructor = None;
        let mut members = Vec::new();
        while !self.eat("}")? {
            if self.eat(";")? {
                continue;
            }
            let mstart = self.tok.start;
            let mline = self.line();
            let mut is_static = false;
            if self.is_kw("static") {
                let next = self.peek()?;
                if !matches!(next.tok, Tok::Punct("(") | Tok::Punct("=") | Tok::Punct(";") | Tok::Punct("}")) {
                    self.advance()?;
                    is_static = true;
                    if self.is("{") {
                        // bloco estático
                        let saved = self.ctx;
                        self.ctx.in_function = true;
                        self.ctx.in_async = false;
                        let body = self.parse_block()?;
                        self.ctx = saved;
                        members.push(ClassMember {
                            key: PropKeyExpr::Name("".into()),
                            is_static: true,
                            kind: MemberKind::StaticBlock(body),
                            func: None,
                        });
                        continue;
                    }
                }
            }
            let mut is_async = false;
            let mut is_gen = false;
            let mut accessor: Option<MemberKind> = None;
            if let Tok::Ident(n) = self.tok.tok.clone()
                && matches!(&*n, "get" | "set" | "async")
            {
                let next = self.peek()?;
                let is_prefix = !matches!(next.tok, Tok::Punct("(") | Tok::Punct("=") | Tok::Punct(";") | Tok::Punct("}"))
                    && !(next.nl_before && &*n == "async");
                if is_prefix {
                    match &*n {
                        "get" => accessor = Some(MemberKind::Getter),
                        "set" => accessor = Some(MemberKind::Setter),
                        _ => is_async = true,
                    }
                    self.advance()?;
                }
            }
            if self.eat("*")? {
                is_gen = true;
            }
            let key = self.parse_prop_key()?;
            let is_ctor = !is_static && matches!(&key, PropKeyExpr::Name(n) if &**n == "constructor");
            if self.is("(") {
                let fkind = match (&accessor, is_ctor) {
                    (_, true) if superclass.is_some() => FnKind::DerivedConstructor,
                    (_, true) => FnKind::ClassConstructor,
                    (Some(MemberKind::Getter), _) => FnKind::Getter,
                    (Some(MemberKind::Setter), _) => FnKind::Setter,
                    _ => FnKind::Method,
                };
                let fname = match &key {
                    PropKeyExpr::Name(n) => Some(n.clone()),
                    PropKeyExpr::Private(n) => Some(format!("#{n}").into()),
                    _ => None,
                };
                let fname = if is_ctor { name.clone() } else { fname };
                let f = self.parse_function_rest(fname, fkind, is_async, is_gen, mstart, mline)?;
                if is_ctor {
                    constructor = Some(f);
                } else {
                    members.push(ClassMember {
                        key,
                        is_static,
                        kind: accessor.unwrap_or(MemberKind::Method),
                        func: Some(f),
                    });
                }
            } else {
                // campo
                let init = if self.eat("=")? {
                    let saved = self.ctx;
                    self.ctx.in_function = true;
                    self.ctx.in_async = false;
                    self.ctx.in_generator = false;
                    let e = self.parse_assign(false)?;
                    self.ctx = saved;
                    Some(e)
                } else {
                    None
                };
                self.semicolon()?;
                members.push(ClassMember {
                    key,
                    is_static,
                    kind: MemberKind::Field(init),
                    func: None,
                });
            }
        }
        self.strict = saved_strict;
        self.ctx = saved_ctx;
        let source = self.lx.source_slice(start, self.prev_end);
        Ok(Rc::new(ClassDef {
            name,
            superclass,
            constructor,
            members,
            source: source.into(),
            line,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) {
        if let Err(e) = parse_program(src) {
            panic!("falhou: {src}\n  linha {}: {}", e.line, e.msg);
        }
    }

    #[test]
    fn sintaxe_moderna() {
        ok("let x = 1; const {a, b: [c, d = 2], ...rest} = obj; var f = (a, b = 1, ...r) => a + b;");
        ok("class A extends B { #p = 1; static s = 2; constructor(x) { super(x); this.#p = x; } get v() { return this.#p; } static { init(); } }");
        ok("async function f() { await g(); for await (const x of xs) {} } function* g() { yield 1; yield* h(); }");
        ok("const t = `a${b}c${`d${e}`}`; tag`x${1}y`; a?.b?.[c]?.(d) ?? e; x ||= 1; y ??= 2; z &&= 3;");
        ok("label: for (let i = 0; i < 10; i++) { if (i) continue label; else break label; }");
        ok("switch (x) { case 1: f(); break; default: g(); } try { a() } catch { b() } finally { c() }");
        ok("for (const [k, v] of Object.entries(o)) {} for (k in o) {} [a, b] = [b, a]; ({x, y = 2} = p);");
        ok("x = a ? b : c => d; f(...args, ...more); new A; new A.B(1)(2); new.target; x = /re[/]g/.test(s) / 2;");
        ok("var a = 1\nvar b = 2\nreturnValue = a\n++b");
        ok("o = { get x() { return 1 }, set x(v) {}, async m() {}, *g() {}, [k]: 1, 'q': 2, 3: 4, x, ...y }");
    }

    #[test]
    fn erros() {
        assert!(parse_program("let = ;").is_err());
        assert!(parse_program("a + ;").is_err());
        assert!(parse_program("function () {}").is_err());
    }
}
