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

/// Pega o estado do hospedeiro guardado na VM.
pub(crate) fn host(vm: &Vm) -> Rc<RefCell<Host>> {
    vm.host
        .clone()
        .and_then(|h| h.downcast::<RefCell<Host>>().ok())
        .expect("VM sem hospedeiro")
}

/// O NodeId de um valor JS que embrulha um nó.
pub(crate) fn node_of(v: &Value) -> Option<NodeId> {
    match v {
        Value::Object(o) => match o.borrow().kind {
            Kind::Host { class: NODE, id } => Some(id),
            _ => None,
        },
        _ => None,
    }
}

/// O objeto JS de um nó (sempre o mesmo para o mesmo nó).
pub(crate) fn wrap(vm: &mut Vm, n: NodeId) -> Value {
    let h = host(vm);
    let mut hb = h.borrow_mut();
    if let Some(o) = hb.wrappers.get(&n) {
        return Value::Object(o.clone());
    }
    let (iface, custom) = dom::interface_of(&hb, n);
    let proto = match custom {
        Some(p) => p,
        None => hb.protos.get(iface).or_else(|| hb.protos.get("HTMLElement")).cloned().expect("protótipos do DOM"),
    };
    let mut obj = ferro_js::value::Object::new(Some(proto), Kind::Host { class: NODE, id: n });
    obj.tag = iface;
    let o = ObjRef::new(obj);
    hb.wrappers.insert(n, o.clone());
    Value::Object(o)
}

pub(crate) fn wrap_opt(vm: &mut Vm, n: Option<NodeId>) -> Value {
    match n {
        Some(n) => wrap(vm, n),
        None => Value::Null,
    }
}

/// Tipo de um `<script>` especial: "module", "importmap" (None para os outros).
fn script_type(doc: &Document, n: NodeId) -> Option<String> {
    let el = doc.element(n)?;
    if el.tag != "script" {
        return None;
    }
    let ty = el.attr("type")?.trim().to_ascii_lowercase();
    matches!(ty.as_str(), "module" | "importmap").then_some(ty)
}

/// Tipos de `<script>` que são JavaScript clássico.
fn is_classic_script(doc: &Document, n: NodeId) -> bool {
    let Some(el) = doc.element(n) else { return false };
    if el.tag != "script" {
        return false;
    }
    let ty = el.attr("type").unwrap_or("").trim().to_ascii_lowercase();
    let lang = el.attr("language").unwrap_or("").trim().to_ascii_lowercase();
    match ty.as_str() {
        "" => lang.is_empty() || lang.starts_with("javascript"),
        "text/javascript" | "application/javascript" | "application/x-javascript" | "text/ecmascript"
        | "application/ecmascript" | "text/jscript" | "text/livescript" => true,
        t => t.starts_with("text/javascript") || t.starts_with("application/javascript"),
    }
}

fn now_ms(vm: &Vm) -> f64 {
    vm.now_ms()
}

pub struct Session {
    pub js: Engine,
    pub host: Rc<RefCell<Host>>,
    last_render: Option<Instant>,
    render_cost: Duration,
    /// Última vez que o usuário mexeu na página (o coletor espera ele parar).
    last_input: Option<Instant>,
}

/// Pilha das threads das abas (o interpretador é recursivo).
pub const STACK_SIZE: usize = 512 << 20;

impl Session {
    pub fn new(page: Loaded, net: Net, fonts: Rc<Fonts>, width: f32, height: f32) -> Session {
        Session::new_frame(page, net, fonts, width, height, 0, String::new())
    }

    /// Sessão de um `<iframe>` (`depth` > 0) ou da página da aba (`depth` = 0).
    pub(crate) fn new_frame(page: Loaded, net: Net, fonts: Rc<Fonts>, width: f32, height: f32, depth: u32, parent_origin: String) -> Session {
        let (tx, rx) = mpsc::channel();
        let mut page = page;
        let sheet_cache = std::mem::take(&mut page.sheet_texts);
        let host = Host {
            page,
            net,
            fonts,
            images: crate::layout::Images::new(),
            image_cache: HashMap::new(),
            canvases: HashMap::new(),
            canvas_dirty: HashSet::new(),
            canvas_snaps: HashMap::new(),
            last_canvas_snap: None,
            bitmaps: HashMap::new(),
            bitmap_seq: 0,
            frames: HashMap::new(),
            frame_loading: HashMap::new(),
            frame_images: HashMap::new(),
            frames_changed: HashSet::new(),
            focused_frame: None,
            frame_depth: 0,
            parent_origin: None,
            outbox: Vec::new(),
            inbox: Vec::new(),
            select_popup: None,
            media_loading: HashMap::new(),
            requested: HashSet::new(),
            img_waiters: HashMap::new(),
            width,
            height,
            scroll_y: 0.0,
            wrappers: HashMap::new(),
            protos: HashMap::new(),
            dirty: true,
            styles_dirty: false,
            repaint: false,
            layout: None,
            started: HashSet::new(),
            parser: None,
            modules: Default::default(),
            import_map: HashMap::new(),
            current_script: None,
            ready_state: "loading",
            navigate: None,
            post: None,
            pool: Pool {
                queue: Arc::new(Mutex::new(VecDeque::new())),
                active: Arc::new(AtomicUsize::new(0)),
                tx,
            },
            rx,
            in_flight: 0,
            fetch_seq: 0,
            fetch_waiters: HashMap::new(),
            raf: Vec::new(),
            raf_seq: 0,
            last_raf: Instant::now(),
            templates: HashMap::new(),
            sheet_cache,
            focused: None,
            keyboard_focus: false,
            hover: None,
            fullscreen: None,
            fullscreen_changed: false,
            dyn_rules: None,
            hooks: HashMap::new(),
            custom: HashMap::new(),
            upgraded: HashSet::new(),
            observing: false,
            closed_shadows: HashSet::new(),
            media: crate::media::Media::new(),
            scroll_offsets: HashMap::new(),
            page_scroll_request: None,
            title_changed: false,
            no_resources: false,
        };
        let mut host = host;
        host.frame_depth = depth;
        host.parent_origin = (depth > 0).then_some(parent_origin);
        let host = Rc::new(RefCell::new(host));
        let mut js = Engine::new();
        js.vm.max_depth = 3000;
        crate::jshost::setup(&mut js);
        js.set_output(|kind, text| match kind {
            Out::Log => eprintln!("[console] {text}"),
            Out::Error => eprintln!("[console:erro] {text}"),
        });
        js.vm.host = Some(host.clone() as Rc<dyn std::any::Any>);
        dom::install(&mut js.vm);
        web::install(&mut js.vm);
        let prelude = include_str!("prelude.js");
        if let Err(e) = js.eval(prelude, "ferro:prelude") {
            eprintln!("[ferro] erro no prelúdio JS: {}", js.error_text(&e));
        }
        // ganchos que o prelúdio deixou para o Rust
        let global = Value::Object(js.vm.realm.global.clone());
        if let Ok(Value::Object(h)) = js.vm.get_str(&global, "__ferroHooks") {
            let keys: Vec<PropKey> = h.borrow().props.ordered_keys();
            for k in keys {
                if let PropKey::Str(name) = &k {
                    let name: &'static str = Box::leak(name.to_rust().into_boxed_str());
                    let v = js.vm.get_obj(&h, &k).unwrap_or(Value::Undefined);
                    host.borrow_mut().hooks.insert(name, v);
                }
            }
            js.vm.delete_prop(&js.vm.realm.global.clone(), &PropKey::str("__ferroHooks"));
        }
        Session {
            js,
            host,
            last_render: None,
            render_cost: Duration::ZERO,
            last_input: None,
        }
    }

    /// Chama uma função do prelúdio; erros viram "Uncaught ..." no console.
    fn hook(&mut self, name: &str, args: Vec<Value>) -> Value {
        let f = self.host.borrow().hooks.get(name).cloned();
        let Some(f) = f else { return Value::Undefined };
        match self.js.vm.call(&f, Value::Undefined, args) {
            Ok(v) => {
                self.js.vm.run_microtasks();
                v
            }
            Err(e) => {
                self.js.vm.report_uncaught(&e);
                Value::Undefined
            }
        }
    }

    /// Roda os `<script>` do documento, na ordem, e dispara DOMContentLoaded.
    pub fn run_scripts(&mut self, cancel: &(dyn Fn() -> bool + Sync)) {
        self.start_scripts();
        while !self.step_scripts(Duration::from_secs(3600)) {
            if cancel() {
                return;
            }
            // espera o próximo script chegar pela rede
            let got = self.host.borrow().rx.recv_timeout(Duration::from_millis(20));
            if let Ok(done) = got {
                self.host.borrow_mut().in_flight -= 1;
                self.on_done(done);
            }
        }
    }

