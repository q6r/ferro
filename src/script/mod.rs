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

/// `Kind::Host { class }` dos nós do DOM (o `id` é o NodeId).
pub const NODE: u32 = 1;

/// Lista de opções de um `<select>` para a interface mostrar.
#[derive(Clone, Debug)]
pub struct SelectPopup {
    pub node: NodeId,
    /// Caixa do `<select>` (documento).
    pub rect: Rect,
    pub labels: Vec<String>,
    pub disabled: Vec<bool>,
    pub selected: i32,
}

/// Corpo de um envio por POST: (content-type, bytes).
pub type PostData = (String, Vec<u8>);

/// Navegação pedida pela página.
pub struct Nav {
    pub url: Url,
    pub post: Option<PostData>,
}

/// Resultado de um trabalho de rede feito em outra thread.
pub(crate) enum Done {
    /// `<script src>` inserido por um script.
    Script { node: NodeId, text: Option<String> },
    /// `fetch`/XMLHttpRequest.
    Fetch { id: u32, res: Result<net::Response, String> },
    Image { url: Url, img: Option<Arc<ImageData>> },
    Bg { url: String, img: Option<Arc<ImageData>> },
    Font,
    Sheet { url: Url, text: Option<String> },
    /// Script externo do HTML (baixado enquanto os anteriores rodam).
    ParserScript { url: Url, text: Option<String> },
    /// Texto de um módulo ES.
    Module { url: Url, text: Option<String> },
    /// Documento de um `<iframe>`.
    Frame { node: NodeId, url: Url, result: Result<Box<Loaded>, String> },
    /// Arquivo de mídia (`<video src>`/`<audio src>`).
    MediaFile { node: NodeId, url: Url, bytes: Option<Vec<u8>> },
}

/// Scripts do HTML ainda rodando (em ordem, um de cada vez).
pub(crate) struct ParserScripts {
    /// Textos dos scripts externos que já chegaram.
    texts: HashMap<Url, Option<String>>,
    requested: HashSet<Url>,
    count: usize,
    started: Instant,
    /// `<script type=module>` do HTML: rodam depois dos clássicos, antes do
    /// DOMContentLoaded (como `defer`).
    modules: Vec<(NodeId, Option<String>, String)>,
    modules_started: bool,
}

type Job = Box<dyn FnOnce() -> Done + Send>;

/// Até 8 threads buscando recursos; o resultado volta por um canal.
struct Pool {
    queue: Arc<Mutex<VecDeque<Job>>>,
    active: Arc<AtomicUsize>,
    tx: mpsc::Sender<Done>,
}

impl Pool {
    fn submit(&self, job: Job) {
        self.queue.lock().unwrap().push_back(job);
        if self.active.load(Ordering::SeqCst) >= 8 {
            return;
        }
        self.active.fetch_add(1, Ordering::SeqCst);
        let (q, a, tx) = (self.queue.clone(), self.active.clone(), self.tx.clone());
        let spawned = std::thread::Builder::new().name("ferro-rede".into()).spawn(move || {
            loop {
                let job = q.lock().unwrap().pop_front();
                match job {
                    Some(job) => {
                        if tx.send(job()).is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            a.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // a página foi embora: o que ainda não começou não precisa mais
        self.queue.lock().unwrap().clear();
    }
}

/// O último layout: retângulos dos elementos e estilos calculados.
pub(crate) struct LayoutInfo {
    pub boxes: Vec<(NodeId, Rect)>,
    /// Áreas de mouse na ordem de pintura (`true` = fixa na janela).
    pub hits: Vec<(NodeId, Rect, bool)>,
    /// Elementos dentro de `position: fixed`.
    pub fixed: std::collections::HashSet<NodeId>,
    pub height: f32,
    pub styles: Styles,
    pub scrollers: Vec<crate::layout::Scroller>,
}

