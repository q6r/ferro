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

/// Tudo o que os scripts de uma página enxergam.
pub struct Host {
    pub page: Loaded,
    pub(crate) net: Net,
    pub(crate) fonts: Rc<Fonts>,
    pub(crate) images: crate::layout::Images,
    /// Imagens já decodificadas, por URL (None = falhou).
    image_cache: HashMap<Url, Option<Arc<ImageData>>>,
    /// Imagem de cada `<canvas>` com contexto 2D.
    pub(crate) canvases: HashMap<NodeId, crate::canvas::Bitmap>,
    /// Canvas desenhados desde a última cópia para a tela.
    pub(crate) canvas_dirty: HashSet<NodeId>,
    /// Última cópia de cada canvas (o que a página mostra).
    canvas_snaps: HashMap<NodeId, Arc<ImageData>>,
    last_canvas_snap: Option<Instant>,
    /// ImageBitmap (createImageBitmap): cópias por id.
    pub(crate) bitmaps: HashMap<u32, Arc<ImageData>>,
    /// Quadros (`<iframe>`) desta página, pelo nó.
    pub(crate) frames: HashMap<NodeId, frames::Child>,
    /// Quadros baixando (nó → endereço).
    pub(crate) frame_loading: HashMap<NodeId, Url>,
    /// Imagem atual de cada quadro e os que mudaram desde a última cópia.
    pub(crate) frame_images: HashMap<NodeId, Arc<ImageData>>,
    pub(crate) frames_changed: HashSet<NodeId>,
    /// Quadro que recebe as teclas (o último clicado).
    pub(crate) focused_frame: Option<NodeId>,
    /// 0 = página da aba; 1 = dentro de um `<iframe>`...
    pub(crate) frame_depth: u32,
    /// Origem da página-mãe (só nos quadros).
    pub(crate) parent_origin: Option<String>,
    /// Arquivos de mídia baixando (elemento → endereço).
    pub(crate) media_loading: HashMap<NodeId, Url>,
    /// `<select>` clicado: a interface abre a lista (nó, rótulos, desabilitadas, escolhida).
    pub(crate) select_popup: Option<(NodeId, Vec<String>, Vec<bool>, i32)>,
    /// Mensagens para a página-mãe e vindas dela.
    pub(crate) outbox: Vec<frames::Message>,
    pub(crate) inbox: Vec<frames::Message>,
    pub(crate) bitmap_seq: u32,
    requested: HashSet<String>,
    /// `<img>` esperando a imagem para disparar `load`/`error`.
    img_waiters: HashMap<Url, Vec<NodeId>>,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) scroll_y: f32,
    pub(crate) wrappers: HashMap<NodeId, ObjRef>,
    pub(crate) protos: HashMap<&'static str, ObjRef>,
    /// O DOM mudou desde o último layout.
    pub(crate) dirty: bool,
    /// Algum `<style>`/`<link>` mudou: recoletar as folhas de estilo.
    pub(crate) styles_dirty: bool,
    /// Chegou recurso (imagem, fonte) que muda o desenho.
    pub(crate) repaint: bool,
    pub(crate) layout: Option<LayoutInfo>,
    /// Scripts que já rodaram (ou que nunca devem rodar, como os do innerHTML).
    pub(crate) started: HashSet<NodeId>,
    /// Scripts do parser ainda rodando (None: acabaram ou não começaram).
    pub(crate) parser: Option<ParserScripts>,
    /// Módulos ES carregados neste documento.
    pub(crate) modules: module::Modules,
    /// `<script type=importmap>`: especificador → URL.
    pub(crate) import_map: HashMap<String, Url>,
    pub(crate) current_script: Option<NodeId>,
    pub(crate) ready_state: &'static str,
    /// Navegação pedida pela página (location.href = ..., clique em link).
    pub(crate) navigate: Option<Url>,
    /// Corpo do POST da navegação pedida (formulário com method=post).
    pub(crate) post: Option<PostData>,
    pool: Pool,
    rx: mpsc::Receiver<Done>,
    in_flight: usize,
    pub(crate) fetch_seq: u32,
    /// Callbacks dos fetch em andamento.
    pub(crate) fetch_waiters: HashMap<u32, Value>,
    pub(crate) raf: Vec<(u32, Value)>,
    pub(crate) raf_seq: u32,
    last_raf: Instant,
    pub(crate) templates: HashMap<NodeId, NodeId>,
    /// Folhas externas já baixadas.
    sheet_cache: HashMap<Url, Option<String>>,
    pub(crate) focused: Option<NodeId>,
    /// O foco veio do teclado (Tab): `:focus-visible` vale para qualquer elemento.
    pub(crate) keyboard_focus: bool,
    /// Elemento sob o mouse.
    pub(crate) hover: Option<NodeId>,
    /// Elemento em tela cheia (requestFullscreen).
    pub(crate) fullscreen: Option<NodeId>,
    /// A tela cheia mudou e a interface ainda não sabe.
    pub(crate) fullscreen_changed: bool,
    /// As folhas de estilo têm `:hover`/`:active`? (None = ainda não olhou.)
    dyn_rules: Option<bool>,
    /// Funções do prelúdio que o Rust chama (disparar eventos etc.).
    pub(crate) hooks: HashMap<&'static str, Value>,
    pub(crate) custom: HashMap<String, ObjRef>,
    /// Elementos personalizados que já rodaram o construtor.
    pub(crate) upgraded: HashSet<NodeId>,
    /// Há MutationObserver ativo (as mutações avisam o prelúdio).
    pub(crate) observing: bool,
    /// Hospedeiros com sombra `mode: "closed"` (shadowRoot devolve null).
    pub(crate) closed_shadows: HashSet<NodeId>,
    /// Media Source Extensions e players dos <video>/<audio>.
    pub(crate) media: crate::media::Media,
    /// Rolagem dos elementos que rolam por dentro.
    pub(crate) scroll_offsets: HashMap<NodeId, (f32, f32)>,
    /// A página pediu para rolar a janela (scrollTo, scrollIntoView).
    pub(crate) page_scroll_request: Option<f32>,
    pub(crate) title_changed: bool,
    no_resources: bool,
}

