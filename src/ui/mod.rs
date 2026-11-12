//! Interface do navegador. Dois layouts (abas na lateral ou em cápsulas),
//! a página num painel flutuante e a busca numa paleta de vidro no centro
//! da tela. Tudo é desenhado pelo próprio Ferro, com o mesmo Canvas que
//! pinta as páginas.

mod chrome;
mod pages;
mod pagetools;
mod painter;
mod textfield;
pub mod theme;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::engine::{EngineHandle, Frame, Update};
use crate::fonts::Fonts;
use crate::layout::Rect;
use crate::net;
use crate::paint::Canvas;
use painter::{Icon, IconCache, Painter, rect};
use textfield::TextField;
use theme::{THEMES, Theme};

const NEW_TAB_URL: &str = "ferro://nova-aba";
const SETTINGS_URL: &str = "ferro://configuracoes";

pub enum UserEvent {
    Engine { tab: u64, update: Update },
    /// Só redesenhar (um aviso que expirou).
    Redraw,
}

/// Modo de captura: desenha a interface fora da tela e salva um PNG
/// (sem abrir janela). Útil para testes e para gerar imagens dos temas.
pub struct Capture {
    pub out: PathBuf,
    pub width: f32,
    pub height: f32,
    pub theme: Option<String>,
    pub layout: Option<String>,
    /// Abre a paleta de busca com este texto.
    pub typed: Option<String>,
    /// Abas extras abertas antes da principal (para mostrar a lista).
    pub extra_tabs: Vec<Url>,
    /// Busca este texto na página (Ctrl+F) antes de capturar.
    pub find: Option<String>,
    /// Seleciona todo o texto da página antes de capturar.
    pub select_all: bool,
    /// Rola a página até aqui (px CSS) antes de capturar.
    pub scroll: Option<f32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutMode {
    /// Trilho de ícones + lista de abas à esquerda (kit 4).
    Sidebar,
    /// Abas em pílulas no topo.
    Capsules,
}

impl LayoutMode {
    fn id(self) -> &'static str {
        match self {
            LayoutMode::Sidebar => "lateral",
            LayoutMode::Capsules => "capsulas",
        }
    }

    fn from_id(s: &str) -> LayoutMode {
        if s.trim() == "capsulas" { LayoutMode::Capsules } else { LayoutMode::Sidebar }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Appearance,
    Layout,
    About,
}

/// Alvos clicáveis da interface (recalculados a cada quadro).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Tab(usize),
    CloseTab(usize),
    NewTab,
    Back,
    Forward,
    Reload,
    Settings,
    Home,
    Search,
    TogglePanel,
    Capsule,
    PaletteInput,
    PaletteRow(usize),
    Scrim,
    Shortcut(usize),
    FakeBox,
    Customize,
    SettingsBack,
    Section(Section),
    Theme(usize),
    Layout(LayoutMode),
    Retry,
    ScrollThumb,
    ScrollTrack,
    FindBar,
    FindClose,
    SelectOption(usize),
    SelectList,
    MenuItem(usize),
    Menu,
}

/// Ações do menu de contexto (botão direito).
#[derive(Clone, PartialEq)]
enum MenuAction {
    OpenLinkTab(String),
    CopyLink(String),
    CopySelection,
    SelectAll,
    Back,
    Forward,
    Reload,
    Find,
    Fullscreen,
}

type Hits = Vec<(Rect, Hit)>;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    NewTab,
    Settings(Section),
    Web,
}

struct Entry {
    url: Url,
    title: String,
    scroll: f32,
}

struct Tab {
    id: u64,
    engine: EngineHandle,
    history: Vec<Entry>,
    index: usize,
    frame: Option<Box<Frame>>,
    nav: u64,
    loading: bool,
    progress: f32,
    /// Rolagem em px CSS.
    scroll: f32,
    /// Ainda não chegou o primeiro quadro da navegação atual.
    fresh: bool,
    /// Âncora a rolar quando a página chegar.
    fragment: Option<String>,
    error: Option<String>,
    /// Última rolagem avisada ao motor (para `window.scrollY`).
    sent_scroll: f32,
    /// Um campo de texto da página tem o foco: as teclas são dela.
    editing: bool,
    /// Corpo do POST para o próximo carregamento (envio de formulário).
    pending_post: Option<crate::script::PostData>,
    /// Ponteiro que a página pede onde o mouse está.
    cursor: crate::style::Cursor,
    /// Última posição do mouse avisada ao motor (coordenadas do documento).
    sent_mouse: Option<(f32, f32)>,
}

impl Tab {
    fn entry(&self) -> &Entry {
        &self.history[self.index]
    }

    fn url(&self) -> &Url {
        &self.entry().url
    }

    fn kind(&self) -> Kind {
        let u = self.url();
        if u.scheme() != "ferro" {
            return Kind::Web;
        }
        match u.host_str() {
            Some("configuracoes") => Kind::Settings(match u.fragment() {
                Some("sobre") => Section::About,
                Some("layout") => Section::Layout,
                _ => Section::Appearance,
            }),
            _ => Kind::NewTab,
        }
    }

    fn title(&self) -> String {
        match self.kind() {
            Kind::NewTab => "Nova aba".into(),
            Kind::Settings(_) => "Configurações".into(),
            Kind::Web => {
                let t = &self.entry().title;
                if !t.is_empty() {
                    t.clone()
                } else {
                    self.url().host_str().unwrap_or(self.url().as_str()).to_string()
                }
            }
        }
    }

    fn icon(&self) -> Icon {
        if self.loading {
            return Icon::Reload;
        }
        match self.kind() {
            Kind::NewTab => Icon::Home,
            Kind::Settings(_) => Icon::Settings,
            Kind::Web if self.error.is_some() => Icon::Warning,
            Kind::Web => Icon::Globe,
        }
    }
}

enum Action {
    Open(Url),
    Tab(usize),
}

struct Suggestion {
    icon: Icon,
    text: String,
    detail: String,
    hint: &'static str,
    group: &'static str,
    action: Action,
}

#[derive(Clone, Copy, PartialEq)]
enum FullscreenBy {
    Page,
    User,
}

/// Onde fica cada parte da interface, em pixels lógicos.
struct Geo {
    rail: Option<Rect>,
    panel: Option<Rect>,
    tabs_row: Option<Rect>,
    bar: Rect,
    capsule: Rect,
    content: Rect,
}

pub struct App {
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Rc<Window>>,
    context: Option<softbuffer::Context<Rc<Window>>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    surface_size: (u32, u32),
    canvas: Canvas,
    fonts: Fonts,
    icons: IconCache,
    scale: f32,
    zoom: f32,
    theme: usize,
    layout: LayoutMode,
    panel_collapsed: bool,
    tabs: Vec<Tab>,
    active: usize,
    next_tab_id: u64,
    next_nav: u64,
    omni: TextField,
    palette_open: bool,
    palette_edited: bool,
    suggestions: Vec<Suggestion>,
    suggestion: usize,
    mods: ModifiersState,
    mouse: (f32, f32),
    hits: Hits,
    hover: Option<Hit>,
    hover_link: Option<String>,
    cursor: CursorIcon,
    cursor_visible: bool,
    scrollbar: Option<(Rect, f32)>,
    drag_scroll: Option<f32>,
    history: Vec<(Url, String)>,
    clipboard: Option<arboard::Clipboard>,
    initial: Option<Url>,
    last_title: String,
    capture: Option<Capture>,
    /// Busca na página e seleção de texto (da aba ativa).
    tools: pagetools::Tools,
    /// Aviso rápido (download concluído...).
    toast: Option<pagetools::Toast>,
    /// Tela cheia: pela página (requestFullscreen) ou pelo F11.
    fullscreen: Option<FullscreenBy>,
    /// Rolagem antes da tela cheia da página (volta ao sair).
    fs_scroll: f32,
    /// Lista aberta de um `<select>` (aba, popup, opção destacada, primeira visível).
    select: Option<(u64, crate::script::SelectPopup, usize, usize)>,
    /// Menu de contexto aberto: posição e itens (rótulo, ação; None = separador).
    menu: Option<((f32, f32), Vec<(&'static str, Option<MenuAction>)>)>,
    started: Instant,
}

/// Abre a janela do navegador (ou, com `capture`, só gera a imagem).
pub fn run(initial: Option<Url>, capture: Option<Capture>) -> Result<(), String> {
    // cookies e localStorage do perfil (a captura não mexe no perfil)
    if capture.is_none()
        && let Some(dir) = crate::dados::default_dir()
    {
        crate::dados::enable(dir);
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    let config = load_config();
    let mut app = App {
        proxy: event_loop.create_proxy(),
        window: None,
        context: None,
        surface: None,
        surface_size: (0, 0),
        canvas: Canvas::default(),
        fonts: Fonts::system()?,
        icons: IconCache::default(),
        scale: 1.0,
        zoom: 1.0,
        theme: config.0,
        layout: config.1,
        panel_collapsed: false,
        tabs: Vec::new(),
        active: 0,
        next_tab_id: 1,
        next_nav: 0,
        omni: TextField::default(),
        palette_open: false,
        palette_edited: false,
        suggestions: Vec::new(),
        suggestion: 0,
        mods: ModifiersState::empty(),
        mouse: (0.0, 0.0),
        hits: Vec::new(),
        hover: None,
        hover_link: None,
        cursor: CursorIcon::Default,
        cursor_visible: true,
        scrollbar: None,
        drag_scroll: None,
        history: Vec::new(),
        clipboard: None,
        initial,
        last_title: String::new(),
        tools: Default::default(),
        toast: None,
        fullscreen: None,
        fs_scroll: 0.0,
        select: None,
        menu: None,
        capture,
        started: Instant::now(),
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())
}

// ---------------------------------------------------------- configuração

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("Ferro").join("config.txt"))
}

fn load_config() -> (usize, LayoutMode) {
    let text = config_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let get = |key: &str| text.lines().find_map(|l| l.strip_prefix(key).map(str::trim));
    (
        get("tema=").map_or(0, theme::by_id),
        get("layout=").map_or(LayoutMode::Sidebar, LayoutMode::from_id),
    )
}

fn save_config(theme: usize, layout: LayoutMode) {
    if let Some(p) = config_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, format!("tema={}\nlayout={}\n", THEMES[theme].id, layout.id()));
    }
}

// --------------------------------------------------------------- endereços

/// Valor `key` do DOM para uma tecla do winit ("a", "Enter", "ArrowLeft"...).
fn dom_key(k: &Key) -> Option<String> {
    match k {
        Key::Character(c) => Some(c.to_string()),
        Key::Named(NamedKey::Space) => Some(" ".into()),
        Key::Named(n) => {
            let name = format!("{n:?}");
            // os nomes do winit seguem a especificação de teclas do W3C
            (!name.contains('(')).then_some(name)
        }
        _ => None,
    }
}

fn looks_like_url(t: &str) -> bool {
    if t.chars().any(char::is_whitespace) {
        return false;
    }
    ["http://", "https://", "file:", "ferro:", "data:"]
        .iter()
        .any(|p| t.to_ascii_lowercase().starts_with(p))
        || t.contains('.')
        || t.starts_with("localhost")
}

fn resolve_url(t: &str) -> Option<Url> {
    if let Ok(u) = Url::parse(t)
        && matches!(u.scheme(), "http" | "https" | "file" | "ferro" | "data")
    {
        return Some(u);
    }
    Url::parse(&format!("https://{t}")).ok()
}

fn search_url(q: &str) -> Url {
    let q: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
    Url::parse(&format!("https://html.duckduckgo.com/html/?q={q}")).unwrap()
}

fn resolve_input(t: &str) -> Option<Url> {
    let t = t.trim();
    if t.is_empty() {
        return None;
    }
    if looks_like_url(t)
        && let Some(u) = resolve_url(t)
    {
        return Some(u);
    }
    Some(search_url(t))
}

fn same_document(a: &Url, b: &Url) -> bool {
    let (mut a, mut b) = (a.clone(), b.clone());
    a.set_fragment(None);
    b.set_fragment(None);
    a == b
}

/// Desfaz os escapes %XX para exibição (só se o resultado for UTF-8 legível).
fn pretty(s: &str) -> String {
    match String::from_utf8(net::percent_decode(s)) {
        Ok(t) if !t.chars().any(char::is_control) => t,
        _ => s.to_string(),
    }
}

/// Endereço para exibir: (domínio, resto).
fn display_url(u: &Url) -> (String, String) {
    match u.scheme() {
        "http" | "https" => {
            let host = u.host_str().unwrap_or("").trim_start_matches("www.").to_string()
                + &u.port().map(|p| format!(":{p}")).unwrap_or_default();
            let mut rest = u.path().to_string();
            if let Some(q) = u.query() {
                rest += &format!("?{q}");
            }
            if rest == "/" {
                rest.clear();
            }
            (host, pretty(&rest))
        }
        "ferro" => (u.as_str().to_string(), String::new()),
        _ => (String::new(), u.as_str().to_string()),
    }
}

