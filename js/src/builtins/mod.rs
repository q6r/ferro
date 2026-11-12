//! Objetos embutidos (Object, Array, String, Math, JSON, console...).

pub mod array;
pub mod async_generator;
pub mod bigint;
pub mod collections;
pub mod console;
pub mod date;
pub mod error;
pub mod function;
pub mod generator;
pub mod global;
pub mod intl;
pub mod json;
pub mod number;
pub mod object;
pub mod promise;
pub mod proxy;
pub mod reflect;
pub mod regexp;
pub mod strings;
pub mod symbol;
pub mod timers;
pub mod typed;

use crate::string::JsStr;
use crate::value::*;
use crate::vm::Vm;

