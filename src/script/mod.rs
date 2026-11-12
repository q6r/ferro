//! Scripts das páginas: o Ferro JS ligado ao DOM.
//!
//! Cada página carregada vira uma [`Session`]: o motor JS, o documento e o
//! que os scripts enxergam (embrulhos dos nós, timers, requisições em
//! andamento). A thread da aba chama [`Session::tick`] no seu laço de
//! eventos e redesenha quando o DOM muda.
//!
//! Divisão do trabalho:
//! - `dom.rs`: Node, Element, Document... em Rust, direto sobre a arena do DOM;
//! - `web.rs`: o resto da plataforma em Rust (rede, URL, armazenamento...);
//! - `prelude.js`: o que é mais natural em JS (eventos, `style`, `classList`,
//!   `dataset`, atributos refletidos, fetch/XHR sobre as funções nativas).

mod canvas;
mod dom;
mod frames;
mod media;
mod module;
mod web;

pub use web::{export_local_storage, import_local_storage};

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use ferro_js::value::{Kind, ObjRef, PropKey};
use ferro_js::{Engine, Out, Value, Vm};
use url::Url;

use crate::dom::{Document, NodeId};
use crate::engine::{self, Loaded};
use crate::fonts::{self, Fonts};
use crate::layout::{Page, Rect};
use crate::net::{self, Net};
use crate::paint::ImageData;
use crate::style::{Cursor, Styles};

