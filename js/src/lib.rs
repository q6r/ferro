//! Ferro JS: motor de JavaScript próprio, 100% Rust.
//!
//! Fluxo: texto → `lexer` → `parser` (AST) → `compiler` (bytecode) → `vm`.
//! Os objetos embutidos (Object, Array, JSON, console...) ficam em `builtins`.

pub mod ast;
pub mod bigint;
pub mod builtins;
pub mod compiler;
pub mod fxhash;
pub mod gc;
pub mod conv;
pub mod lexer;
pub mod module;
pub mod ops;
pub mod parser;
pub mod regex;
pub mod resolve;
pub mod string;
pub mod value;
pub mod vm;

pub use string::JsStr;
pub use value::{ObjRef, Value};
pub use vm::{Out, Vm};

/// Interface simples para quem embute o motor (o navegador, a linha de comando).
pub struct Engine {
    pub vm: Vm,
}

impl Default for Engine {
