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

