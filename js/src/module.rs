//! Módulos ES (`import`/`export`).
//!
//! Um módulo vira uma função assíncrona comum:
//!
//! ```text
//! (async function (__ns, __m0, __m1, ..., __meta) {
//!     __ferroExport(__ns, "x", () => x);   // um getter por nome exportado
//!     ...corpo do módulo...
//! })
//! ```
//!
//! `__ns` é o namespace do próprio módulo (o objeto que quem importa vê);
//! `__mN` são os namespaces dos módulos pedidos, na ordem de `requests`.
//! Um nome importado (`import { a as b } from "x"`) é compilado como
//! `__mN.a` em cada uso: a ligação fica viva, como o spec pede (ciclos
//! entre módulos funcionam). O `await` no topo vale porque a função é
//! assíncrona. Quem roda o módulo (o navegador) cria os namespaces, baixa
//! as dependências e chama as funções na ordem certa.

use std::rc::Rc;

use crate::ast::*;
use crate::compiler::{self, Code};
use crate::fxhash::FxHashMap;
use crate::parser::{self, ParseError};
use crate::value::Value;
use crate::vm::Vm;

/// Um módulo compilado: os especificadores pedidos (na ordem dos
/// parâmetros `__m0`, `__m1`...) e o código que cria a função do módulo.
pub struct CompiledModule {
    pub requests: Vec<String>,
    pub code: Rc<Code>,
}

/// String JS com aspas (para montar o prólogo).
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Nomes que uma declaração exportada cria.
fn declared_names(s: &Stmt, out: &mut Vec<Name>) {
    match s {
        Stmt::Var(_, decls, _) => {
            for d in decls {
                compiler::bound_names(&d.target, out);
            }
        }
        Stmt::Function(f) => out.extend(f.name.clone()),
        Stmt::Class(c) => out.extend(c.name.clone()),
        _ => {}
    }
}

/// Analisa e compila o texto de um módulo. `url` vira o nome do arquivo
/// (mensagens de erro, `import()` relativo).
pub fn compile_module(src: &str, url: &str) -> Result<CompiledModule, ParseError> {
    let items = parser::parse_module(src)?;
    let mut requests: Vec<String> = Vec::new();
    let mut request = |spec: &str| -> Name {
        let i = match requests.iter().position(|r| r == spec) {
            Some(i) => i,
            None => {
                requests.push(spec.to_string());
                requests.len() - 1
            }
        };
        Name::from(format!("__m{i}").as_str())
    };
    let mut imports: FxHashMap<Name, (Name, Option<Name>)> = FxHashMap::default();
    let mut body: Vec<Stmt> = Vec::new();
    // prólogo: getters do namespace e reexportações
    let mut prologue = String::new();
    for item in items {
        match item {
            ModuleItem::Stmt(s) => body.push(s),
            ModuleItem::Import(d) => {
                let ns = request(&d.specifier);
                for (what, local) in d.bindings {
                    let export = match what {
                        ImportName::Default => Some(Name::from("default")),
                        ImportName::Named(n) => Some(Name::from(n.as_str())),
                        ImportName::Namespace => None,
                    };
                    imports.insert(local, (ns.clone(), export));
                }
            }
            ModuleItem::Export(e) => match e {
                ExportDecl::Decl(s) => {
                    let mut names = Vec::new();
                    declared_names(&s, &mut names);
                    for n in names {
                        prologue.push_str(&format!("__ferroExport(__ns, {}, () => {n});\n", quote(&n)));
                    }
                    body.push(s);
                }
                ExportDecl::DefaultDecl(s, n) => {
                    prologue.push_str(&format!("__ferroExport(__ns, \"default\", () => {n});\n"));
                    body.push(s);
                }
                ExportDecl::DefaultExpr(e) => {
                    prologue.push_str("__ferroExport(__ns, \"default\", () => __default);\n");
                    body.push(Stmt::Var(
                        VarKind::Const,
                        vec![VarDecl {
                            target: Pattern::Ident(Name::from("__default")),
                            init: Some(e),
                        }],
                        0,
                    ));
                }
                ExportDecl::Names(list) => {
                    for (local, exported) in list {
                        prologue.push_str(&format!("__ferroExport(__ns, {}, () => {local});\n", quote(&exported)));
                    }
                }
                ExportDecl::From(spec, list) => {
                    let ns = request(&spec);
                    for (theirs, exported) in list {
                        prologue.push_str(&format!(
                            "__ferroExport(__ns, {}, () => {ns}[{}]);\n",
                            quote(&exported),
                            quote(&theirs)
                        ));
                    }
                }
                ExportDecl::All(spec, alias) => {
                    let ns = request(&spec);
                    match alias {
                        Some(a) => prologue.push_str(&format!("__ferroExport(__ns, {}, () => {ns});\n", quote(&a))),
                        None => prologue.push_str(&format!("__ferroExportAll(__ns, {ns});\n")),
                    }
                }
            },
        }
    }
    let mut params = String::from("__ns");
    for i in 0..requests.len() {
        params.push_str(&format!(", __m{i}"));
    }
    params.push_str(", __meta");
    // a casca: uma função assíncrona com o prólogo; o corpo entra depois
    let shell = format!("(async function module({params}) {{\n\"use strict\";\n{prologue}}})");
    let mut prog = parser::parse_program(&shell)?;
    let Some(Stmt::Expr(Expr::Function(f), _)) = prog.body.first_mut() else {
        return Err(ParseError { msg: "módulo: casca inválida".into(), line: 0, eof: false });
    };
    let Some(def) = Rc::get_mut(f) else {
        return Err(ParseError { msg: "módulo: casca compartilhada".into(), line: 0, eof: false });
    };
    def.strict = true;
    def.source = Rc::from("[módulo]");
    if let FnBody::Block(b) = &mut def.body {
        b.extend(body);
    }
    prog.strict = true;
    prog.imports = imports;
    let code = compiler::compile_script(&prog, url)?;
    Ok(CompiledModule { requests, code })
}

/// Compila e devolve a função do módulo (ou SyntaxError).
pub fn module_function(vm: &mut Vm, src: &str, url: &str) -> Result<(Vec<String>, Value), Value> {
    match compile_module(src, url) {
        Ok(m) => {
            let f = vm.run_script(m.code)?;
            Ok((m.requests, f))
        }
        Err(e) => Err(vm.syntax_error(format!("{} ({url}:{})", e.msg, e.line))),
    }
}
