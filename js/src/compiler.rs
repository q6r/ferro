//! Compilador: AST → bytecode de uma máquina de pilha.
//!
//! Os nomes são resolvidos em tempo de compilação. Variáveis que nenhuma
//! função interna usa ficam em registradores do quadro (rápido); as que são
//! capturadas por closures ficam em ambientes no heap, acessados por
//! (saltos, posição). O que não é encontrado é global.
//!
//! `finally` é copiado em cada saída (break/continue/return) e no caminho de
//! exceção, então a VM não precisa de um mecanismo especial para ele.

use std::cell::RefCell;
use std::collections::HashSet;

use crate::fxhash::{FxHashMap, FxHashSet};
use std::rc::Rc;

use crate::ast::*;
use crate::parser::ParseError;
use crate::resolve::{Captures, Resolver, class_init_key, key_of, slice_key};
use crate::string::JsStr;
use crate::value::{ObjRef, Value};

pub type CResult<T> = Result<T, ParseError>;

/// Tipos de método para DefineMethod.
pub const METHOD: u8 = 0;
pub const GETTER: u8 = 1;
pub const SETTER: u8 = 2;

/// Tipos de erro para ThrowError.
pub const ERR_TYPE: u8 = 0;
pub const ERR_SYNTAX: u8 = 1;
pub const ERR_REFERENCE: u8 = 2;

