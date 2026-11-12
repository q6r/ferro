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
                    let cons = self.parse_statement()?;
                    let alt = if self.eat_kw("else")? { Some(Box::new(self.parse_statement()?)) } else { None };
                    Ok(Stmt::If(test, Box::new(cons), alt))
                }
                "for" => self.parse_for(),
                "while" => {
                    self.advance()?;
                    self.expect("(")?;
                    let test = self.parse_expression(false)?;
                    self.expect(")")?;
                    let body = self.parse_statement()?;
                    Ok(Stmt::While(test, Box::new(body)))
                }
                "do" => {
                    self.advance()?;
                    let body = self.parse_statement()?;
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
                "with" => self.err("'with' não é suportado"),
                _ => {
                    // rótulo?
                    if !RESERVED.contains(&&*kw) && matches!(self.peek()?.tok, Tok::Punct(":")) {
                        self.advance()?;
                        self.advance()?;
                        let body = self.parse_statement()?;
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
                    let body = Box::new(self.parse_statement()?);
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
                    let body = Box::new(self.parse_statement()?);
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
        let body = Box::new(self.parse_statement()?);
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

