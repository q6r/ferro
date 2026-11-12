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

    /// Começa a rodar os scripts do HTML: pede todos os externos em paralelo.
    /// Depois, [`Session::step_scripts`] roda os que já chegaram, em ordem.
    pub fn start_scripts(&mut self) {
        // import maps primeiro (valem para os módulos)
        let maps: Vec<String> = {
            let hb = self.host.borrow();
            let doc = &hb.page.doc;
            doc.descendants(Document::ROOT)
                .into_iter()
                .filter(|&n| script_type(doc, n).as_deref() == Some("importmap"))
                .map(|n| doc.text_content(n))
                .collect()
        };
        for m in maps {
            module::add_import_map(&mut self.js.vm, &m);
        }
        let mut hb = self.host.borrow_mut();
        let srcs: Vec<Url> = {
            let doc = &hb.page.doc;
            doc.descendants(Document::ROOT)
                .into_iter()
                .filter(|&n| is_classic_script(doc, n))
                .filter_map(|n| doc.element(n)?.attr("src").and_then(|s| hb.page.base.join(s.trim()).ok()))
                .collect()
        };
        let modules: Vec<(NodeId, Option<String>, String)> = {
            let doc = &hb.page.doc;
            doc.descendants(Document::ROOT)
                .into_iter()
                .filter(|&n| script_type(doc, n).as_deref() == Some("module"))
                .map(|n| (n, doc.element(n).and_then(|e| e.attr("src")).map(str::to_string), doc.text_content(n)))
                .collect()
        };
        let mut parser = ParserScripts {
            texts: HashMap::new(),
            requested: HashSet::new(),
            count: 0,
            started: Instant::now(),
            modules: modules.clone(),
            modules_started: false,
        };
        for u in srcs {
            if parser.requested.insert(u.clone()) {
                request_parser_script(&mut hb, u);
            }
        }
        hb.parser = Some(parser);
        let base = hb.page.base.clone();
        drop(hb);
        // os módulos externos já começam a baixar (com as dependências)
        for (_, src, _) in &modules {
            if let Some(u) = src.as_deref().and_then(|s| base.join(s.trim()).ok()) {
                module::request(&mut self.js.vm, &u);
            }
        }
    }

    /// Ainda há scripts do HTML para rodar?
    pub fn scripts_pending(&self) -> bool {
        self.host.borrow().parser.is_some()
    }

    /// Roda os scripts do HTML, em ordem, até o tempo `budget` passar ou o
    /// próximo ainda não ter chegado. `true` quando todos rodaram (e o
    /// DOMContentLoaded já disparou).
    pub fn step_scripts(&mut self, budget: Duration) -> bool {
        let t = Instant::now();
        loop {
            if self.host.borrow().parser.is_none() {
                return true;
            }
            // o próximo script do parser que ainda não rodou (document.write
            // pode ter acrescentado outros)
            let next = {
                let hb = self.host.borrow();
                let doc = &hb.page.doc;
                doc.descendants(Document::ROOT)
                    .into_iter()
                    .find(|&n| doc.tag(n) == Some("script") && !hb.started.contains(&n))
            };
            let Some(n) = next else {
                // os clássicos acabaram: agora os módulos do HTML, na ordem
                let pending = {
                    let mut hb = self.host.borrow_mut();
                    let p = hb.parser.as_mut().unwrap();
                    if p.modules_started { Vec::new() } else {
                        p.modules_started = true;
                        p.modules.clone()
                    }
                };
                for (m, src, text) in pending {
                    module::start_script(&mut self.js.vm, m, src, &text);
                }
                module::drive(&mut self.js.vm);
                let waiting = {
                    let hb = self.host.borrow();
                    hb.parser.as_ref().unwrap().modules.iter().any(|(m, _, _)| !module::script_done(&hb, *m))
                };
                if waiting {
                    return false;
                }
                self.scripts_finished();
                return true;
            };
            let (classic, src, inline) = {
                let hb = self.host.borrow();
                let doc = &hb.page.doc;
                let src = doc
                    .element(n)
                    .and_then(|e| e.attr("src"))
                    .and_then(|s| hb.page.base.join(s.trim()).ok());
                (is_classic_script(doc, n), src, doc.text_content(n))
            };
            if !classic {
                self.host.borrow_mut().started.insert(n);
                continue;
            }
            let (text, name) = match src {
                Some(u) => {
                    let text = {
                        let mut hb = self.host.borrow_mut();
                        let hb = &mut *hb;
                        let parser = hb.parser.as_mut().unwrap();
                        match parser.texts.remove(&u) {
                            Some(t) => Some(t),
                            None => {
                                if parser.requested.insert(u.clone()) {
                                    request_parser_script(hb, u.clone());
                                }
                                None
                            }
                        }
                    };
                    // ainda não chegou: volta ao laço de eventos e espera
                    let Some(text) = text else { return false };
                    self.host.borrow_mut().started.insert(n);
                    let Some(text) = text else {
                        let el = wrap(&mut self.js.vm, n);
                        self.hook("fire", vec![el, Value::str("error")]);
                        continue;
                    };
                    (text, u.to_string())
                }
                None => {
                    let mut hb = self.host.borrow_mut();
                    hb.started.insert(n);
                    let count = hb.parser.as_ref().map_or(0, |p| p.count);
                    (inline, format!("{}#script{count}", hb.page.url))
                }
            };
            if let Some(p) = self.host.borrow_mut().parser.as_mut() {
                p.count += 1;
            }
            self.exec_script(n, &text, &name);
            if self.host.borrow().page.doc.element(n).and_then(|e| e.attr("src")).is_some() {
                let el = wrap(&mut self.js.vm, n);
                self.hook("fire", vec![el, Value::str("load")]);
            }
            if t.elapsed() >= budget {
                return false;
            }
        }
    }

    /// Todos os scripts do HTML rodaram: DOMContentLoaded.
    fn scripts_finished(&mut self) {
        let parser = {
            let mut hb = self.host.borrow_mut();
            hb.ready_state = "interactive";
            hb.parser.take()
        };
        let doc = wrap(&mut self.js.vm, Document::ROOT);
        self.hook("fire", vec![doc.clone(), Value::str("readystatechange")]);
        self.hook("fire", vec![doc, Value::str("DOMContentLoaded"), Value::Bool(true)]);
        if std::env::var_os("FERRO_TEMPOS").is_some()
            && let Some(p) = parser
        {
            eprintln!("[ferro] scripts: {} em {:.0} ms", p.count, p.started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    /// Fim do carregamento (depois das imagens): `load` na janela.
    pub fn finish_load(&mut self) {
        self.host.borrow_mut().ready_state = "complete";
        let doc = wrap(&mut self.js.vm, Document::ROOT);
        self.hook("fire", vec![doc, Value::str("readystatechange")]);
        let win = Value::Object(self.js.vm.realm.global.clone());
        self.hook("fire", vec![win.clone(), Value::str("load")]);
        self.hook("fire", vec![win, Value::str("pageshow")]);
    }

    fn exec_script(&mut self, n: NodeId, text: &str, name: &str) {
        exec_script(&mut self.js.vm, n, text, name);
        self.js.vm.run_microtasks();
        // depuração: FERRO_VIGIA="expressão" avisa quando o valor muda
        if let Ok(code) = std::env::var("FERRO_VIGIA") {
            let v = match self.js.vm.eval_script(&code, "vigia") {
                Ok(v) => self.js.inspect(&v),
                Err(e) => self.js.error_text(&e),
            };
            thread_local!(static LAST: RefCell<String> = const { RefCell::new(String::new()) });
            LAST.with(|l| {
                if *l.borrow() != v {
                    eprintln!("[vigia] depois de {name}: {v}");
                    *l.borrow_mut() = v;
                }
            });
        }
    }

    /// Estilo + layout do documento atual. Guarda os retângulos (para cliques
    /// e `getBoundingClientRect`) e pede o que faltar (imagens, fontes).
    pub fn render(&mut self) -> Page {
        let t = Instant::now();
        let mut page = render_host(&self.host);
        // quadros: descobre os novos, redesenha os que mudaram de tamanho
        self.discover_frames();
        self.paint_frames(false);
        {
            let hb = self.host.borrow();
            for (&n, c) in &hb.frames {
                if let Some(r) = frames::frame_box(&hb, n) {
                    let fixed = hb.layout.as_ref().is_some_and(|l| l.fixed.contains(&n));
                    page.scrollers.push(crate::layout::Scroller {
                        node: n,
                        rect: r,
                        x: 0.0,
                        y: c.scroll,
                        max_x: 0.0,
                        max_y: (c.height - c.size.1).max(0.0),
                        fixed,
                    });
                }
            }
        }
        self.render_cost = t.elapsed();
        self.last_render = Some(Instant::now());
        page
    }

    /// Há algo novo para desenhar e já passou tempo suficiente do último desenho?
    pub fn wants_frame(&self) -> bool {
        let hb = self.host.borrow();
        if !(hb.dirty || hb.repaint || hb.layout.is_none()) {
            return false;
        }
        // não redesenha mais que ~1 vez a cada 2× o custo do desenho (mín. 30 ms)
        let gap = (self.render_cost * 2).max(Duration::from_millis(30));
        self.last_render.is_none_or(|t| t.elapsed() >= gap)
    }

    /// Quanto o laço da aba pode dormir até ter trabalho de JS.
    pub fn wakeup(&self) -> Option<Duration> {
        let hb = self.host.borrow();
        let mut best: Option<f64> = None;
        let mut take = |ms: f64| best = Some(best.map_or(ms, |b: f64| b.min(ms)));
        let now = self.js.vm.now_ms();
        for t in &self.js.vm.timers {
            take((t.due - now).max(0.0));
        }
        if !hb.raf.is_empty() {
            take(16.0 - hb.last_raf.elapsed().as_secs_f64() * 1000.0);
        }
        if hb.in_flight > 0 {
            take(8.0);
        }
        if hb.parser.is_some() {
            take(4.0);
        }
        if hb.dirty || hb.repaint {
            take(30.0);
        }
        if hb.media.active() {
            take(10.0);
        }
        best.map(|ms| Duration::from_secs_f64(ms.max(0.0) / 1000.0))
    }

    /// Uma volta do laço de eventos: rede, timers vencidos, animação.
    pub fn tick(&mut self) {
        let start = Instant::now();
        let log = std::env::var_os("FERRO_MIDIA_LOG").is_some();
        // respostas da rede
        loop {
            let done = self.host.borrow().rx.try_recv();
            let Ok(done) = done else { break };
            self.host.borrow_mut().in_flight -= 1;
            self.on_done(done);
        }
        let t_net = start.elapsed();
        // timers vencidos (com limite, para não travar a aba)
        let mut n = 0;
        while n < 200 && start.elapsed() < Duration::from_millis(50) && self.js.vm.run_one_timer(false) {
            n += 1;
        }
        let t_timers = start.elapsed();
        // mídia: relógio e eventos dos <video>/<audio>
        let media_events = self.host.borrow_mut().media.tick();
        for (n, ty) in media_events {
            let el = wrap(&mut self.js.vm, n);
            self.hook("fire", vec![el, Value::str(ty)]);
        }
        // requestAnimationFrame
        let due = {
            let hb = self.host.borrow();
            !hb.raf.is_empty() && hb.last_raf.elapsed() >= Duration::from_millis(16)
        };
        if due {
            let list = {
                let mut hb = self.host.borrow_mut();
                hb.last_raf = Instant::now();
                std::mem::take(&mut hb.raf)
            };
            let ts = Value::Number(now_ms(&self.js.vm));
            for (_, cb) in list {
                if let Err(e) = self.js.vm.call(&cb, Value::Undefined, vec![ts.clone()]) {
                    self.js.vm.report_uncaught(&e);
                }
            }
            self.js.vm.run_microtasks();
        }
        // mensagens da página-mãe e os quadros desta página
        if !self.host.borrow().inbox.is_empty() {
            self.deliver_inbox();
        }
        self.tick_frames();
        // coletor de ciclos, entre uma tarefa e outra; com o usuário mexendo
        // na página, só se a memória estiver crescendo muito
        let busy = self.last_input.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
        let collect = if busy { ferro_js::gc::should_collect_urgent() } else { ferro_js::gc::should_collect() };
        if collect && self.host.borrow().frame_depth == 0 {
            let st = ferro_js::gc::collect();
            if std::env::var_os("FERRO_TEMPOS").is_some() {
                eprintln!(
                    "[ferro] coletor: {} examinados, {} liberados em {:.0} ms",
                    st.examined, st.freed, st.ms
                );
            }
        }
        if log && start.elapsed().as_millis() > 80 {
            eprintln!(
                "[mídia] volta lenta: rede {:.0} ms, timers {:.0} ms ({n}), total {:.0} ms",
                t_net.as_secs_f64() * 1000.0,
                (t_timers - t_net).as_secs_f64() * 1000.0,
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    fn on_done(&mut self, done: Done) {
        match done {
            Done::ParserScript { url, text } => {
                if let Some(p) = self.host.borrow_mut().parser.as_mut() {
                    p.texts.insert(url, text);
                }
            }
            Done::Module { url, text } => {
                module::arrived(&mut self.js.vm, url, text);
                module::drive(&mut self.js.vm);
            }
            Done::MediaFile { node, url, bytes } => {
                let ok = {
                    let mut hb = self.host.borrow_mut();
                    // o elemento pode ter trocado de endereço enquanto baixava
                    if hb.media_loading.get(&node) != Some(&url) {
                        return;
                    }
                    hb.media_loading.remove(&node);
                    match bytes.map(|b| hb.media.load_file(&b)) {
                        Some(Ok(source)) => {
                            hb.media.attach(node, source);
                            true
                        }
                        Some(Err(e)) => {
                            if std::env::var_os("FERRO_MIDIA_LOG").is_some() {
                                eprintln!("[mídia] {url}: {e}");
                            }
                            false
                        }
                        None => false,
                    }
                };
                let el = wrap(&mut self.js.vm, node);
                self.hook("mediaFile", vec![el, Value::Bool(ok)]);
            }
            Done::Frame { node, url, result } => {
                // o <iframe> pode ter mudado de endereço enquanto baixava
                let current = self.host.borrow().frame_loading.get(&node) == Some(&url);
                if !current {
                    return;
                }
                match result {
                    Ok(page) => self.attach_frame(node, *page),
                    Err(_) => {
                        self.host.borrow_mut().frame_loading.remove(&node);
                        let el = wrap(&mut self.js.vm, node);
                        self.hook("fire", vec![el, Value::str("load")]);
                    }
                }
            }
            Done::Script { node, text } => {
                let el = wrap(&mut self.js.vm, node);
                match text {
                    Some(text) => {
                        let name = self
                            .host
                            .borrow()
                            .page
                            .doc
                            .element(node)
                            .and_then(|e| e.attr("src"))
                            .unwrap_or("script")
                            .to_string();
                        self.exec_script(node, &text, &name);
                        self.hook("fire", vec![el, Value::str("load")]);
                    }
                    None => {
                        self.hook("fire", vec![el, Value::str("error")]);
                    }
                }
            }
            Done::Fetch { id, res } => {
                let cb = self.host.borrow_mut().fetch_waiters.remove(&id);
                if let Some(cb) = cb {
                    let arg = web::response_value(&mut self.js.vm, res);
                    if let Err(e) = self.js.vm.call(&cb, Value::Undefined, vec![arg]) {
                        self.js.vm.report_uncaught(&e);
                    }
                    self.js.vm.run_microtasks();
                }
            }
            Done::Image { url, img } => {
                let ok = img.is_some();
                let waiters = {
                    let mut hb = self.host.borrow_mut();
                    hb.image_cache.insert(url.clone(), img);
                    hb.repaint = true;
                    hb.img_waiters.remove(&url).unwrap_or_default()
                };
                for n in waiters {
                    let el = wrap(&mut self.js.vm, n);
                    self.hook("fire", vec![el, Value::str(if ok { "load" } else { "error" })]);
                }
            }
            Done::Bg { url, img } => {
                let mut hb = self.host.borrow_mut();
                if let Some(img) = img {
                    hb.images.urls.insert(url, img);
                }
                hb.repaint = true;
            }
            Done::Font => self.host.borrow_mut().repaint = true,
            Done::Sheet { url, text } => {
                let mut hb = self.host.borrow_mut();
                hb.sheet_cache.insert(url, text);
                hb.styles_dirty = true;
                hb.dirty = true;
            }
        }
    }

    /// Clique na página (coordenadas do documento). Devolve a URL a abrir,
    /// se o clique seguiu um link.
    pub fn click(&mut self, x: f32, y: f32) -> Option<Nav> {
        self.last_input = Some(Instant::now());
        self.host.borrow_mut().keyboard_focus = false;
        let target = self.hit(x, y)?;
        // clique dentro de um <iframe>: vai para o quadro
        if let Some((n, fx, fy)) = self.frame_at(target, x, y) {
            self.host.borrow_mut().focused_frame = Some(n);
            let c = self.host.borrow_mut().frames.remove(&n);
            if let Some(mut c) = c {
                if let Some(nav) = c.session.click(fx, fy) {
                    c.session.host.borrow_mut().navigate = Some(nav.url);
                }
                c.stale = true;
                self.host.borrow_mut().frames.insert(n, c);
            }
            return None;
        }
        self.host.borrow_mut().focused_frame = None;
        let el = wrap(&mut self.js.vm, target);
        let scroll = self.host.borrow().scroll_y;
        let ok = self.hook(
            "click",
            vec![el, Value::Number(x as f64), Value::Number((y - scroll) as f64)],
        );
        let nav = if ferro_js::vm::truthy(&ok) { self.default_click(target) } else { None };
        match nav {
            Some(url) => Some(Nav { url, post: None }),
            None => self.take_navigation(),
        }
    }

    /// Ação padrão de um clique não cancelado.
    fn default_click(&mut self, target: NodeId) -> Option<Url> {
        let mut hb = self.host.borrow_mut();
        let mut n = Some(target);
        while let Some(id) = n {
            let doc = &hb.page.doc;
            if let Some(el) = doc.element(id) {
                match el.tag.as_str() {
                    "a" | "area" => {
                        if let Some(href) = el.attr("href") {
                            let href = href.trim();
                            if href.to_ascii_lowercase().starts_with("javascript:") {
                                let code = net::percent_decode(&href[11..]);
                                let code = String::from_utf8_lossy(&code).into_owned();
                                drop(hb);
                                if let Err(e) = self.js.vm.eval_script(&code, "javascript:") {
                                    self.js.vm.report_uncaught(&e);
                                }
                                self.js.vm.run_microtasks();
                                return self.host.borrow_mut().navigate.take();
                            }
                            return hb.page.base.join(href).ok();
                        }
                    }
                    "input" => {
                        let ty = el.attr("type").unwrap_or("").to_ascii_lowercase();
                        if ty == "checkbox" {
                            if el.attr("checked").is_some() {
                                hb.page.doc.remove_attr(id, "checked");
                            } else {
                                hb.page.doc.set_attr(id, "checked", "");
                            }
                            hb.dirty = true;
                            return None;
                        }
                        if ty == "radio" {
                            hb.page.doc.set_attr(id, "checked", "");
                            hb.dirty = true;
                            return None;
                        }
                    }
                    _ => {}
                }
            }
            n = hb.page.doc.node(id).parent;
        }
        None
    }

    /// O elemento por cima de tudo sob o ponto (coordenadas do documento):
    /// a última área de mouse pintada ali.
    pub fn hit(&self, x: f32, y: f32) -> Option<NodeId> {
        let hb = self.host.borrow();
        let info = hb.layout.as_ref()?;
        let doc = &hb.page.doc;
        let sy = hb.scroll_y;
        info.hits
            .iter()
            .rev()
            .find(|(n, r, fixed)| {
                // os fixos estão em coordenadas da janela
                let py = if *fixed { y - sy } else { y };
                r.contains(x, py) && *n < doc.nodes.len() && doc.is_connected(*n)
            })
            .map(|(n, _, _)| *n)
    }

    /// O mouse andou sobre a página (coordenadas do documento) ou saiu dela
    /// (`None`). Dispara mouseover/out/enter/leave/move e atualiza `:hover`.
    /// Devolve o ponteiro que o CSS pede para o ponto.
    pub fn mouse_move(&mut self, point: Option<(f32, f32)>) -> Cursor {
        self.last_input = Some(Instant::now());
        let target = point.and_then(|(x, y)| self.hit(x, y));
        // sobre um <iframe>: o quadro decide o ponteiro (e recebe o mouse)
        if let (Some(t), Some((x, y))) = (target, point)
            && let Some((n, fx, fy)) = self.frame_at(t, x, y)
        {
            let c = self.host.borrow_mut().frames.remove(&n);
            if let Some(mut c) = c {
                let cursor = c.session.mouse_move(Some((fx, fy)));
                if c.session.wants_frame() {
                    c.stale = true;
                }
                self.host.borrow_mut().frames.insert(n, c);
                return cursor;
            }
        }
        let prev = self.host.borrow().hover;
        if target != prev {
            let mut hb = self.host.borrow_mut();
            hb.hover = target;
            if hb.dyn_rules.is_none() {
                let uses = hb.page.css.iter().any(|c| c.text.contains(":hover") || c.text.contains(":active"));
                hb.dyn_rules = Some(uses);
            }
            if hb.dyn_rules == Some(true) {
                hb.dirty = true;
            }
            crate::css::set_dynamic_state(dynamic_state(&hb));
        }
        let (x, y) = point.unwrap_or((0.0, 0.0));
        if target.is_some() || prev.is_some() {
            let scroll = self.host.borrow().scroll_y;
            let connected = |s: &Self, n: NodeId| {
                let hb = s.host.borrow();
                n < hb.page.doc.nodes.len() && hb.page.doc.is_connected(n)
            };
            let t = match target {
                Some(n) => wrap(&mut self.js.vm, n),
                None => Value::Null,
            };
            let p = match prev.filter(|&n| target != prev && connected(self, n)) {
                Some(n) => wrap(&mut self.js.vm, n),
                None => Value::Null,
            };
            self.hook(
                "move",
                vec![t, Value::Number(x as f64), Value::Number((y - scroll) as f64), p, Value::Bool(target != prev)],
            );
        }
        self.cursor_at(target)
    }

    /// Ponteiro pedido pelo CSS para o elemento.
    fn cursor_at(&self, target: Option<NodeId>) -> Cursor {
        let hb = self.host.borrow();
        let (Some(mut n), Some(info)) = (target, hb.layout.as_ref()) else { return Cursor::Auto };
        let doc = &hb.page.doc;
        for _ in 0..64 {
            if let Some(Some(st)) = info.styles.nodes.get(n) {
                return st.cursor;
            }
            match doc.flat_parent(n) {
                Some(p) => n = p,
                None => break,
            }
        }
        Cursor::Auto
    }

    /// Tecla pressionada na página (`key` no formato do DOM: "a", "Enter"...).
    pub fn key(&mut self, key: &str, text: Option<&str>, ctrl: bool, shift: bool, alt: bool, meta: bool) {
        self.last_input = Some(Instant::now());
        // teclas para o quadro com o foco
        let ff = self.host.borrow().focused_frame;
        if let Some(n) = ff {
            let c = self.host.borrow_mut().frames.remove(&n);
            if let Some(mut c) = c {
                c.session.key(key, text, ctrl, shift, alt, meta);
                c.stale = true;
                self.host.borrow_mut().frames.insert(n, c);
                return;
            }
        }
        if key == "Tab" {
            self.host.borrow_mut().keyboard_focus = true;
        }
        let text = text.map_or(Value::Null, Value::str);
        self.hook(
            "key",
            vec![
                Value::str(key),
                text,
                Value::Bool(ctrl),
                Value::Bool(shift),
                Value::Bool(alt),
                Value::Bool(meta),
            ],
        );
    }

    /// Há um campo de texto com o foco? (A interface deixa as teclas com a página.)
    pub fn editing(&mut self) -> bool {
        let ff = self.host.borrow().focused_frame;
        if let Some(n) = ff {
            let c = self.host.borrow_mut().frames.remove(&n);
            if let Some(mut c) = c {
                let e = c.session.editing();
                self.host.borrow_mut().frames.insert(n, c);
                return e;
            }
        }
        ferro_js::vm::truthy(&self.hook("editing", Vec::new()))
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        {
            let mut hb = self.host.borrow_mut();
            hb.width = width;
            hb.height = height;
            hb.dirty = true;
        }
        let win = Value::Object(self.js.vm.realm.global.clone());
        self.hook("fire", vec![win, Value::str("resize")]);
    }

    pub fn set_scroll(&mut self, y: f32) {
        self.last_input = Some(Instant::now());
        let changed = {
            let mut hb = self.host.borrow_mut();
            let c = (hb.scroll_y - y).abs() > 0.5;
            hb.scroll_y = y;
            c
        };
        if changed {
            let doc = wrap(&mut self.js.vm, Document::ROOT);
            self.hook("fire", vec![doc, Value::str("scroll"), Value::Bool(true)]);
        }
    }

    /// Quadros de vídeo vão direto para `sink` (a interface), de outra thread.
    pub fn set_frame_sink(&mut self, sink: crate::media::player::FrameSink) {
        self.host.borrow_mut().media.frame_sink = Some(sink);
    }

    /// Quadro atual de cada `<video>` que já tem imagem (e a imagem de cada `<canvas>`).
    pub fn video_frames(&mut self) -> HashMap<NodeId, Arc<ImageData>> {
        self.snapshot_canvases();
        let hb = self.host.borrow();
        let mut out: HashMap<NodeId, Arc<ImageData>> = hb
            .media
            .players
            .iter()
            .filter_map(|(&n, p)| p.pipeline.shown.as_ref().map(|(_, img)| (n, img.clone())))
            .collect();
        for (n, img) in &hb.canvas_snaps {
            out.insert(*n, img.clone());
        }
        for (n, img) in &hb.frame_images {
            out.insert(*n, img.clone());
        }
        out
    }

    /// Copia os canvas que mudaram. Devolve as cópias novas.
    fn snapshot_canvases(&mut self) -> Vec<(NodeId, Arc<ImageData>)> {
        let mut hb = self.host.borrow_mut();
        if hb.canvas_dirty.is_empty() {
            return Vec::new();
        }
        hb.last_canvas_snap = Some(Instant::now());
        let dirty: Vec<NodeId> = hb.canvas_dirty.drain().collect();
        let mut out = Vec::new();
        for n in dirty {
            let Some(img) = hb.canvases.get(&n).map(|b| Arc::new(b.snapshot())) else { continue };
            hb.canvas_snaps.insert(n, img.clone());
            out.push((n, img));
        }
        out
    }

    /// Canvas redesenhados (no máximo ~60 vezes por segundo): as imagens novas
    /// vão para a tela sem refazer o layout.
    pub fn take_canvas_frames(&mut self) -> Vec<(NodeId, Arc<ImageData>)> {
        let mut out: Vec<(NodeId, Arc<ImageData>)> = {
            let mut hb = self.host.borrow_mut();
            let changed: Vec<NodeId> = hb.frames_changed.drain().collect();
            changed.into_iter().filter_map(|n| hb.frame_images.get(&n).map(|i| (n, i.clone()))).collect()
        };
        out.extend(self.take_canvas_images());
        out
    }

    fn take_canvas_images(&mut self) -> Vec<(NodeId, Arc<ImageData>)> {
        let due = self
            .host
            .borrow()
            .last_canvas_snap
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(16));
        if !due {
            return Vec::new();
        }
        let connected = {
            let hb = self.host.borrow();
            hb.canvas_dirty.iter().any(|&n| hb.page.doc.is_connected(n))
        };
        if !connected {
            return Vec::new();
        }
        self.snapshot_canvases()
    }

    /// Um `<select>` pediu a lista: (nó, retângulo no documento, rótulos,
    /// desabilitadas, escolhida).
    pub fn take_select_popup(&mut self) -> Option<SelectPopup> {
        let (n, labels, disabled, selected) = self.host.borrow_mut().select_popup.take()?;
        let rect = {
            let hb = self.host.borrow();
            let r = frames::frame_box(&hb, n)?;
            let fixed = hb.layout.as_ref().is_some_and(|l| l.fixed.contains(&n));
            Rect { y: if fixed { r.y + hb.scroll_y } else { r.y }, ..r }
        };
        Some(SelectPopup { node: n, rect, labels, disabled, selected })
    }

    /// A pessoa escolheu a opção `index` no `<select>`.
    pub fn choose_select(&mut self, node: NodeId, index: usize) {
        self.last_input = Some(Instant::now());
        let el = wrap(&mut self.js.vm, node);
        self.hook("selectChoose", vec![el, Value::Number(index as f64)]);
    }

    /// A página entrou ou saiu da tela cheia desde a última consulta?
    pub fn take_fullscreen(&mut self) -> Option<bool> {
        let mut hb = self.host.borrow_mut();
        if !std::mem::take(&mut hb.fullscreen_changed) {
            return None;
        }
        Some(hb.fullscreen.is_some())
    }

    /// A interface saiu da tela cheia (Esc): avisa a página.
    pub fn exit_fullscreen(&mut self) {
        let el = {
            let mut hb = self.host.borrow_mut();
            hb.dirty = true;
            hb.fullscreen_changed = false;
            hb.fullscreen.take()
        };
        if let Some(n) = el {
            crate::css::set_dynamic_state(dynamic_state(&self.host.borrow()));
            let el = wrap(&mut self.js.vm, n);
            self.hook("fullscreenChanged", vec![el]);
        }
    }

    /// `<video controls>` parados (a pintura põe o botão de tocar).
    pub fn paused_videos(&self) -> std::collections::HashSet<NodeId> {
        let hb = self.host.borrow();
        let doc = &hb.page.doc;
        doc.find_all("video")
            .into_iter()
            .filter(|&n| doc.element(n).is_some_and(|e| e.attr("controls").is_some()))
            .filter(|n| hb.media.players.get(n).is_none_or(|p| p.paused))
            .collect()
    }

    /// Algum vídeo mudou de quadro desde a última consulta? Devolve os quadros novos.
    pub fn take_new_frames(&mut self) -> Vec<(NodeId, Arc<ImageData>)> {
        let mut hb = self.host.borrow_mut();
        let ids: Vec<NodeId> = hb.media.new_frames.drain().collect();
        ids.into_iter()
            .filter_map(|n| hb.media.players.get(&n)?.pipeline.shown.as_ref().map(|(_, i)| (n, i.clone())))
            .collect()
    }

    /// Não baixa imagens, fundos nem fontes (modo `--sem-imagens`).
    pub fn disable_resources(&mut self) {
        self.host.borrow_mut().no_resources = true;
    }

    /// Nada baixando e nenhum desenho pendente.
    pub fn idle(&self) -> bool {
        let hb = self.host.borrow();
        hb.in_flight == 0
            && !hb.dirty
            && !hb.repaint
            && hb.parser.is_none()
            && hb.frame_loading.is_empty()
            && hb.frames.values().all(|c| c.loaded)
    }

    /// Rola um elemento por dentro (roda do mouse sobre ele). `false` se ele
    /// já está no limite nessa direção.
    pub fn scroll_element(&mut self, node: NodeId, dx: f32, dy: f32) -> bool {
        if self.host.borrow().frames.contains_key(&node) {
            return self.scroll_frame(node, dy);
        }
        let moved = {
            let mut hb = self.host.borrow_mut();
            let info = hb.layout.as_ref().and_then(|l| l.scrollers.iter().find(|s| s.node == node).copied());
            let Some(sc) = info else { return false };
            let cur = hb.scroll_offsets.get(&node).copied().unwrap_or((sc.x, sc.y));
            let next = ((cur.0 + dx).clamp(0.0, sc.max_x), (cur.1 + dy).clamp(0.0, sc.max_y));
            if (next.0 - cur.0).abs() < 0.5 && (next.1 - cur.1).abs() < 0.5 {
                false
            } else {
                hb.scroll_offsets.insert(node, next);
                hb.dirty = true;
                true
            }
        };
        if moved {
            let el = wrap(&mut self.js.vm, node);
            self.hook("fire", vec![el, Value::str("scroll")]);
        }
        moved
    }

    /// Rolagem da janela pedida pela página (a interface aplica).
    pub fn take_page_scroll(&mut self) -> Option<f32> {
        self.host.borrow_mut().page_scroll_request.take()
    }

    /// Navegação pedida pela página.
    pub fn take_navigation(&mut self) -> Option<Nav> {
        let mut hb = self.host.borrow_mut();
        let url = hb.navigate.take()?;
        Some(Nav { url, post: hb.post.take() })
    }
}

/// Executa o texto de um `<script>` com `document.currentScript` apontando para ele.
pub(crate) fn exec_script(vm: &mut Vm, n: NodeId, text: &str, name: &str) {
    let h = host(vm);
    let prev = h.borrow_mut().current_script.replace(n);
    if let Err(e) = vm.eval_script(text, name) {
        vm.report_uncaught(&e);
    }
    h.borrow_mut().current_script = prev;
}

/// Coleta de novo as folhas de estilo (depois que `<style>`/`<link>` mudaram).
fn recollect_styles(h: &Rc<RefCell<Host>>) {
    let mut hb = h.borrow_mut();
    hb.styles_dirty = false;
    let mut out = Vec::new();
    let mut fetch = Vec::new();
    {
        let doc = &hb.page.doc;
        let mut all = doc.descendants(Document::ROOT);
        for (&host, &root) in &doc.shadow_roots {
            if doc.is_connected(host) {
                all.extend(doc.descendants(root));
            }
        }
        for n in all {
            let Some(el) = doc.element(n) else { continue };
            let media = el.attr("media").map(str::to_string);
            match el.tag.as_str() {
                "style" => out.push(engine::StyleSource {
                    text: engine::absolutize_css_urls(&doc.text_content(n), &hb.page.base),
                    media,
                }),
                "link" => {
                    let rel = el.attr("rel").unwrap_or("").to_ascii_lowercase();
                    let rels: Vec<&str> = rel.split_whitespace().collect();
                    if rels.contains(&"stylesheet")
                        && !rels.contains(&"alternate")
                        && el.attr("disabled").is_none()
                        && let Some(u) = el.attr("href").and_then(|h| hb.page.base.join(h.trim()).ok())
                    {
                        match hb.sheet_cache.get(&u) {
                            Some(Some(text)) => out.push(engine::StyleSource {
                                text: engine::absolutize_css_urls(text, &u),
                                media,
                            }),
                            Some(None) => {}
                            None => fetch.push(u),
                        }
                    }
                }
                _ => {}
            }
        }
    }
    hb.page.css = out;
    hb.dyn_rules = None;
    for u in fetch {
        if !hb.requested.insert(format!("css {u}")) {
            continue;
        }
        let net = hb.net.clone();
        hb.in_flight += 1;
        hb.pool.submit(Box::new(move || {
            let text = match net.fetch(&u) {
                Ok(r) if r.status < 400 => Some(net::decode_text(&r.body, r.content_type.as_deref(), false)),
                _ => None,
            };
            Done::Sheet { url: u, text }
        }));
    }
}

/// Pede uma imagem (para desenhar e/ou para o `load` de um `<img>`).
pub(crate) fn request_image(hb: &mut Host, url: Url, waiter: Option<NodeId>) {
    if let Some(n) = waiter {
        hb.img_waiters.entry(url.clone()).or_default().push(n);
    }
    if hb.image_cache.contains_key(&url) {
        // já está pronta: o `load` sai na próxima volta do laço
        if waiter.is_some() {
            let img = hb.image_cache.get(&url).cloned().flatten();
            hb.in_flight += 1;
            hb.pool.submit(Box::new(move || Done::Image { url, img }));
        }
        return;
    }
    if !hb.requested.insert(format!("img {url}")) {
        return;
    }
    let net = hb.net.clone();
    hb.in_flight += 1;
    hb.pool.submit(Box::new(move || {
        let img = net
            .fetch(&url)
            .ok()
            .filter(|r| r.status < 400)
            .and_then(|r| engine::decode_image(&r.body))
            .map(Arc::new);
        Done::Image { url, img }
    }));
}

/// Cadeia de um nó até a raiz pela árvore achatada (o nó primeiro).
fn flat_chain(doc: &Document, n: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = Some(n);
    while let Some(c) = cur {
        out.push(c);
        cur = doc.flat_parent(c);
    }
    out
}

/// Onde estão o mouse e o foco, para os seletores `:hover`, `:focus`...
fn dynamic_state(hb: &Host) -> crate::css::DynState {
    let doc = &hb.page.doc;
    let live = |n: &NodeId| *n < doc.nodes.len() && doc.is_connected(*n);
    let hover = hb.hover.filter(live).map(|n| flat_chain(doc, n)).unwrap_or_default();
    let focus = hb.focused.filter(live);
    let text_field = focus.is_some_and(|f| {
        doc.element(f).is_some_and(|e| {
            e.tag == "textarea"
                || e.attr("contenteditable").is_some()
                || (e.tag == "input"
                    && !matches!(
                        e.attr("type").unwrap_or("").to_ascii_lowercase().as_str(),
                        "button" | "submit" | "reset" | "checkbox" | "radio" | "image" | "file" | "color" | "range"
                    ))
        })
    });
    crate::css::DynState {
        hover,
        active: Vec::new(),
        focus,
        focus_visible: focus.is_some() && (hb.keyboard_focus || text_field),
        focus_within: focus.map(|f| flat_chain(doc, f)).unwrap_or_default(),
        fullscreen: hb.fullscreen.filter(live),
    }
}

/// Baixa um script do HTML numa thread do conjunto.
fn request_parser_script(hb: &mut Host, u: Url) {
    let net = hb.net.clone();
    hb.in_flight += 1;
    hb.pool.submit(Box::new(move || {
        let text = match net.fetch(&u) {
            Ok(r) if r.status < 400 => Some(net::decode_text(&r.body, r.content_type.as_deref(), false)),
            _ => None,
        };
        Done::ParserScript { url: u, text }
    }));
}

pub(crate) fn render_host(h: &Rc<RefCell<Host>>) -> Page {
    if h.borrow().styles_dirty {
        recollect_styles(h);
    }
    let mut hb = h.borrow_mut();
    // imagens dos <img> que já estão no cache
    let sources = engine::image_sources(&hb.page.doc, &hb.page.base);
    let mut missing = Vec::new();
    for (n, u) in sources {
        match hb.image_cache.get(&u) {
            Some(Some(img)) => {
                let img = img.clone();
                hb.images.nodes.insert(n, img);
            }
            Some(None) => {}
            None => missing.push(u),
        }
    }
    if !hb.no_resources {
        for u in missing {
            request_image(&mut hb, u, None);
        }
    }
    let fonts = hb.fonts.clone();
    crate::layout::set_focus(hb.focused.filter(|&f| hb.page.doc.is_connected(f)));
    crate::css::set_dynamic_state(dynamic_state(&hb));
    crate::layout::set_scroll_offsets(hb.scroll_offsets.clone());
    let (mut page, styles) = engine::render_styled(&hb.page, &hb.images, &fonts, hb.width, hb.height);
    if hb.no_resources {
        page.missing_urls.clear();
        page.missing_fonts.clear();
    }
    // fundos e fontes web que faltam
    for u in std::mem::take(&mut page.missing_urls) {
        if hb.images.urls.contains_key(&u) || !hb.requested.insert(format!("bg {u}")) {
            continue;
        }
        let net = hb.net.clone();
        hb.in_flight += 1;
        hb.pool.submit(Box::new(move || {
            let img = Url::parse(&u)
                .ok()
                .and_then(|url| net.fetch(&url).ok())
                .filter(|r| r.status < 400)
                .and_then(|r| engine::decode_image(&r.body))
                .map(Arc::new);
            Done::Bg { url: u, img }
        }));
    }
    for face in std::mem::take(&mut page.missing_fonts) {
        if !hb.requested.insert(format!("font {}", face.sources[0])) {
            continue;
        }
        let net = hb.net.clone();
        hb.in_flight += 1;
        hb.pool.submit(Box::new(move || {
            for src in &face.sources {
                if fonts::web_font_tried(src) {
                    break;
                }
                let Ok(url) = Url::parse(src) else { continue };
                let Ok(res) = net.fetch(&url) else { continue };
                if res.status < 400
                    && res.body.len() < 4 * 1024 * 1024
                    && fonts::register_web_font(src, &face.family, face.weight, face.italic, &res.body)
                {
                    break;
                }
            }
            Done::Font
        }));
    }
    hb.layout = Some(LayoutInfo {
        boxes: page.boxes.clone(),
        hits: std::mem::take(&mut page.hits),
        fixed: std::mem::take(&mut page.fixed_nodes),
        height: page.height,
        styles,
        scrollers: page.scrollers.clone(),
    });
    hb.dirty = false;
    hb.repaint = false;
    page
}

/// Garante que o layout está em dia (para medidas pedidas por scripts).
pub(crate) fn ensure_layout(h: &Rc<RefCell<Host>>) {
    let stale = {
        let hb = h.borrow();
        hb.dirty || hb.layout.is_none()
    };
    if stale {
        let _ = render_host(h);
        // o desenho para a tela ainda precisa sair
        h.borrow_mut().repaint = true;
    }
}

#[cfg(test)]
mod tests;
