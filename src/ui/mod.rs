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

fn anchor_y(frame: &Frame, fragment: &str) -> Option<f32> {
    let name = String::from_utf8_lossy(&net::percent_decode(fragment)).into_owned();
    if name.is_empty() || name == "top" {
        return Some(0.0);
    }
    frame.anchors.iter().find(|(n, _)| *n == name).map(|(_, y)| *y)
}

impl App {
    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn theme(&self) -> &'static Theme {
        &THEMES[self.theme]
    }

    /// Tamanho da janela em pixels lógicos.
    fn logical_size(&self) -> (f32, f32) {
        if let Some(c) = &self.capture {
            return (c.width, c.height);
        }
        match &self.window {
            Some(w) => {
                let s = w.inner_size();
                (s.width as f32 / self.scale, s.height as f32 / self.scale)
            }
            None => (1280.0, 860.0),
        }
    }

    fn panel_visible(&self, w: f32) -> bool {
        self.layout == LayoutMode::Sidebar && !self.panel_collapsed && w >= 860.0
    }

    fn geo(&self) -> Geo {
        let (w, h) = self.logical_size();
        // tela cheia: só a página, sem moldura
        if self.fullscreen.is_some() {
            let all = rect(0.0, 0.0, w.max(1.0), h.max(1.0));
            return Geo {
                rail: None,
                panel: None,
                tabs_row: None,
                bar: rect(0.0, -100.0, 0.0, 0.0),
                capsule: rect(0.0, -100.0, 0.0, 0.0),
                content: all,
            };
        }
        let (rail, panel, tabs_row, bar, content) = match self.layout {
            LayoutMode::Sidebar => {
                let rail = rect(6.0, 6.0, 58.0, h - 12.0);
                let show = self.panel_visible(w);
                let col = if show { 312.0 } else { 70.0 };
                let panel = show.then(|| rect(70.0, 0.0, 236.0, h));
                let bar = rect(col, 6.0, w - col - 8.0, 44.0);
                let content = rect(col, 54.0, w - col - 8.0, h - 62.0);
                (Some(rail), panel, None, bar, content)
            }
            LayoutMode::Capsules => {
                let tabs = rect(0.0, 6.0, w, 36.0);
                let bar = rect(8.0, 46.0, w - 16.0, 44.0);
                let content = rect(8.0, 94.0, w - 16.0, h - 102.0);
                (None, None, Some(tabs), bar, content)
            }
        };
        let nav_w = 116.0;
        let cap_w = (bar.w - 2.0 * nav_w).clamp(160.0, 660.0);
        let cap_x = (bar.x + bar.w / 2.0 - cap_w / 2.0).max(bar.x + nav_w);
        let cap_w = cap_w.min(bar.x + bar.w - cap_x - 4.0);
        Geo {
            rail,
            panel,
            tabs_row,
            bar,
            capsule: rect(cap_x, bar.y + 4.0, cap_w, 36.0),
            content: rect(content.x, content.y, content.w.max(1.0), content.h.max(1.0)),
        }
    }

    /// Área da página em px CSS (descontando a interface e o zoom).
    fn viewport_css(&self) -> (f32, f32) {
        let c = self.geo().content;
        ((c.w / self.zoom).max(1.0), (c.h / self.zoom).max(1.0))
    }

    fn sync_viewport(&self) {
        if !self.tabs.is_empty() {
            let (vw, vh) = self.viewport_css();
            self.tabs[self.active].engine.resize(vw, vh);
        }
    }

    fn max_scroll(&self, i: usize) -> f32 {
        let vh = self.viewport_css().1;
        self.tabs[i].frame.as_ref().map_or(0.0, |f| (f.height - vh).max(0.0))
    }

    fn effective_scroll(&self, i: usize) -> f32 {
        self.tabs[i].scroll.clamp(0.0, self.max_scroll(i))
    }

    // ----------------------------------------------------------- abas

    fn new_tab(&mut self, url: Option<Url>, activate: bool) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let proxy = self.proxy.clone();
        let (vw, vh) = self.viewport_css();
        let engine = EngineHandle::spawn(
            move |update| {
                let _ = proxy.send_event(UserEvent::Engine { tab: id, update });
            },
            vw,
            vh,
        );
        let url = url.unwrap_or_else(|| Url::parse(NEW_TAB_URL).unwrap());
        let tab = Tab {
            id,
            engine,
            history: vec![Entry {
                url,
                title: String::new(),
                scroll: 0.0,
            }],
            index: 0,
            frame: None,
            nav: 0,
            loading: false,
            progress: 0.0,
            scroll: 0.0,
            fresh: false,
            fragment: None,
            error: None,
            sent_scroll: -1.0,
            editing: false,
            cursor: crate::style::Cursor::Auto,
            sent_mouse: None,
            pending_post: None,
        };
        let idx = if activate || self.tabs.is_empty() {
            self.tabs.len()
        } else {
            (self.active + 1).min(self.tabs.len())
        };
        self.tabs.insert(idx, tab);
        self.load(idx);
        if activate {
            self.activate(idx);
        }
        self.request_redraw();
    }

    fn close_tab(&mut self, i: usize, el: &ActiveEventLoop) {
        if self.tabs.len() <= 1 {
            el.exit();
            return;
        }
        self.tabs.remove(i);
        if i < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        let a = self.active;
        self.activate(a);
    }

    fn activate(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.active = i;
        self.close_palette();
        self.hover_link = None;
        self.sync_viewport();
        self.request_redraw();
    }

    /// Começa a carregar a entrada atual do histórico da aba `i`.
    fn load(&mut self, i: usize) {
        self.next_nav += 1;
        let nav = self.next_nav;
        let tab = &mut self.tabs[i];
        tab.error = None;
        let url = tab.url().clone();
        if url.scheme() == "ferro" {
            tab.engine.stop();
            tab.frame = None;
            tab.loading = false;
            return;
        }
        tab.nav = nav;
        tab.sent_scroll = -1.0;
        tab.editing = false;
        tab.loading = true;
        tab.progress = 0.12;
        tab.fresh = true;
        tab.fragment = url.fragment().map(str::to_string);
        let post = tab.pending_post.take();
        tab.engine.navigate_post(nav, url, post);
    }

    fn navigate(&mut self, i: usize, url: Url) {
        let scroll_now = self.effective_scroll(i);
        let tab = &mut self.tabs[i];
        tab.history[tab.index].scroll = scroll_now;
        tab.history.truncate(tab.index + 1);
        // mesmo documento, só muda o fragmento: rola sem recarregar
        if let Some(frame) = &tab.frame
            && let Some(frag) = url.fragment()
            && tab.kind() == Kind::Web
            && same_document(tab.url(), &url)
        {
            let y = anchor_y(frame, frag);
            let title = tab.entry().title.clone();
            tab.history.push(Entry { url, title, scroll: 0.0 });
            tab.index += 1;
            if let Some(y) = y {
                tab.scroll = y;
            }
            self.request_redraw();
            return;
        }
        tab.history.push(Entry {
            url,
            title: String::new(),
            scroll: 0.0,
        });
        tab.index += 1;
        self.load(i);
        if i == self.active {
            self.close_palette();
        }
        self.request_redraw();
    }

    fn go(&mut self, delta: i32) {
        let i = self.active;
        let scroll_now = self.effective_scroll(i);
        let tab = &mut self.tabs[i];
        let ni = tab.index as i32 + delta;
        if ni < 0 || ni >= tab.history.len() as i32 {
            return;
        }
        tab.history[tab.index].scroll = scroll_now;
        let old = tab.url().clone();
        tab.index = ni as usize;
        let new = tab.url().clone();
        if tab.frame.is_some() && new.scheme() != "ferro" && same_document(&old, &new) {
            tab.scroll = tab.entry().scroll;
        } else {
            self.load(i);
        }
        self.close_palette();
        self.request_redraw();
    }

    fn reload(&mut self) {
        let i = self.active;
        let s = self.effective_scroll(i);
        let tab = &mut self.tabs[i];
        tab.history[tab.index].scroll = s;
        self.load(i);
        self.request_redraw();
    }

    fn stop(&mut self) {
        let tab = &mut self.tabs[self.active];
        tab.engine.stop();
        tab.loading = false;
        self.request_redraw();
    }

    fn open_internal(&mut self, url: &str) {
        let url = Url::parse(url).unwrap();
        let host = url.host_str().map(str::to_string);
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.url().scheme() == "ferro" && t.url().host_str() == host.as_deref())
        {
            self.activate(i);
        } else if self.tab().kind() == Kind::NewTab {
            self.navigate(self.active, url);
        } else {
            self.new_tab(Some(url), true);
        }
    }

    fn go_home(&mut self) {
        if self.tab().kind() != Kind::NewTab {
            self.navigate(self.active, Url::parse(NEW_TAB_URL).unwrap());
        }
    }

    fn open_link(&mut self, href: &str, background: bool) {
        let Some(frame) = &self.tab().frame else { return };
        let Ok(url) = frame.base.join(href.trim()) else { return };
        if !matches!(url.scheme(), "http" | "https" | "file" | "data") {
            return;
        }
        if background {
            self.new_tab(Some(url), false);
        } else {
            self.navigate(self.active, url);
        }
    }

    fn remember(&mut self, url: &Url, title: &str) {
        self.history.retain(|(u, _)| u != url);
        self.history.insert(0, (url.clone(), title.to_string()));
        self.history.truncate(300);
    }

    fn set_layout(&mut self, layout: LayoutMode) {
        self.layout = layout;
        save_config(self.theme, self.layout);
        self.sync_viewport();
        self.request_redraw();
    }

    fn toggle_panel(&mut self) {
        self.panel_collapsed = !self.panel_collapsed;
        self.sync_viewport();
        self.request_redraw();
    }

    fn on_update(&mut self, tab_id: u64, update: Update) {
        let Some(i) = self.tabs.iter().position(|t| t.id == tab_id) else { return };
        if let Update::SelectPopup { nav, popup } = update {
            if nav == self.tabs[i].nav && i == self.active {
                let sel = popup.selected.max(0) as usize;
                self.select = Some((nav, popup, sel, sel.saturating_sub(4)));
                self.request_redraw();
            }
            return;
        }
        if let Update::Fullscreen { nav, on } = update {
            if nav == self.tabs[i].nav && i == self.active {
                if on {
                    self.set_fullscreen(Some(FullscreenBy::Page));
                } else if self.fullscreen == Some(FullscreenBy::Page) {
                    // a página saiu sozinha: não precisa avisá-la de volta
                    let y = self.fs_scroll;
                    self.fullscreen = None;
                    if let Some(w) = &self.window {
                        w.set_fullscreen(None);
                    }
                    self.sync_viewport();
                    self.scroll_to(y);
                }
            }
            return;
        }
        if let Update::Downloaded { nav, keep, name, result } = update {
            let tab = &mut self.tabs[i];
            if nav != tab.nav {
                return;
            }
            tab.loading = false;
            tab.progress = 1.0;
            match keep {
                // a página anterior continua: desfaz a entrada no histórico
                Some(k) => {
                    if tab.index > 0 && tab.history.len() == tab.index + 1 {
                        tab.history.pop();
                        tab.index -= 1;
                    }
                    tab.nav = k;
                }
                None => {
                    tab.error = Some(match &result {
                        Ok(p) => format!("Arquivo baixado em {}", p.display()),
                        Err(e) => format!("Não foi possível baixar {name}: {e}"),
                    });
                }
            }
            self.toast = Some(pagetools::Toast {
                text: match &result {
                    Ok(p) => format!("{name} baixado em {}", p.parent().map(|d| d.display().to_string()).unwrap_or_default()),
                    Err(e) => format!("Falha ao baixar {name}: {e}"),
                },
                since: Instant::now(),
            });
            let proxy = self.proxy.clone();
            // redesenha quando o aviso some
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(5100));
                let _ = proxy.send_event(UserEvent::Redraw);
            });
            self.request_redraw();
            return;
        }
        // o ponteiro muda sem redesenhar nada
        if let Update::Cursor { nav, cursor } = update {
            if nav == self.tabs[i].nav {
                self.tabs[i].cursor = cursor;
                if i == self.active {
                    let (x, y) = self.mouse;
                    self.update_hover(x, y);
                }
            }
            return;
        }
        let mut remember = None;
        let mut go_to = None;
        let mut refind = false;
        {
            let tab = &mut self.tabs[i];
            match update {
                Update::Editing { nav, editing } if nav == tab.nav => tab.editing = editing,
                Update::ScrollTo { nav, y } if nav == tab.nav => {
                    tab.scroll = y;
                }
                Update::VideoFrames { nav, frames } if nav == tab.nav => {
                    if let Some(f) = tab.frame.as_mut() {
                        for (n, img) in frames {
                            f.videos.insert(n, img);
                        }
                    }
                }
                Update::Navigate { nav, url, post } if nav == tab.nav => {
                    if matches!(url.scheme(), "http" | "https" | "file" | "data") {
                        tab.pending_post = post;
                        go_to = Some(url);
                    }
                }
                Update::Started { nav, .. } if nav == tab.nav => tab.progress = 0.3,
                Update::Frame { nav, frame } if nav == tab.nav => {
                    let first = std::mem::take(&mut tab.fresh);
                    let complete = frame.complete;
                    let entry = &mut tab.history[tab.index];
                    let mut url = frame.url.clone();
                    if url.fragment().is_none() {
                        url.set_fragment(entry.url.fragment());
                    }
                    entry.url = url;
                    entry.title = frame.title.clone();
                    if first {
                        remember = Some((entry.url.clone(), entry.title.clone()));
                        tab.scroll = entry.scroll;
                    }
                    if let Some(frag) = tab.fragment.clone()
                        && let Some(y) = anchor_y(&frame, &frag)
                    {
                        tab.scroll = y;
                    }
                    if complete {
                        tab.fragment = None;
                    }
                    tab.loading = !complete;
                    tab.progress = if complete { 1.0 } else { 0.75 };
                    tab.error = None;
                    tab.frame = Some(frame);
                    refind = i == self.active && self.tools.find_open;
                }
                Update::Failed { nav, error, .. } if nav == tab.nav => {
                    tab.loading = false;
                    tab.frame = None;
                    tab.error = Some(error);
                }
                _ => return, // resultado de uma navegação antiga
            }
        }
        if let Some((u, t)) = remember {
            self.remember(&u, &t);
        }
        if refind {
            self.refresh_find(false);
        }
        if let Some(url) = go_to {
            self.navigate(i, url);
            return;
        }
        // a paleta aberta e ainda não editada mostra as abas como estão agora
        if self.palette_open && !self.palette_edited {
            let sel = self.suggestion;
            self.refresh_suggestions();
            self.suggestion = sel.min(self.suggestions.len().saturating_sub(1));
        }
        self.request_redraw();
    }

    // ------------------------------------------------------------- paleta

    /// Abre a paleta de busca. Com `with_url`, começa com o endereço atual
    /// selecionado (como clicar na barra de endereço).
    fn open_palette(&mut self, with_url: bool) {
        let text = if with_url && self.tab().kind() != Kind::NewTab {
            self.tab().url().to_string()
        } else {
            String::new()
        };
        self.palette_open = true;
        self.palette_edited = false;
        self.omni.set(&text);
        self.refresh_suggestions();
        self.request_redraw();
    }

    fn close_palette(&mut self) {
        if self.palette_open {
            self.palette_open = false;
            self.suggestions.clear();
            self.request_redraw();
        }
    }

    fn refresh_suggestions(&mut self) {
        self.suggestions.clear();
        self.suggestion = 0;
        let text = self.omni.text.trim().to_string();
        let host_of = |u: &Url| u.host_str().unwrap_or("").trim_start_matches("www.").to_string();

        if text.is_empty() || !self.palette_edited {
            // estado vazio: abas abertas e recentes
            for (i, t) in self.tabs.iter().enumerate() {
                if i == self.active || self.suggestions.len() >= 5 {
                    continue;
                }
                self.suggestions.push(Suggestion {
                    icon: t.icon(),
                    text: t.title(),
                    detail: host_of(t.url()),
                    hint: "Ir para a aba",
                    group: "Abas abertas",
                    action: Action::Tab(i),
                });
            }
            for (u, title) in self.history.iter().take(5) {
                self.suggestions.push(Suggestion {
                    icon: Icon::Clock,
                    text: if title.is_empty() { u.to_string() } else { title.clone() },
                    detail: host_of(u),
                    hint: "",
                    group: "Recentes",
                    action: Action::Open(u.clone()),
                });
            }
            return;
        }

        if looks_like_url(&text)
            && let Some(u) = resolve_url(&text)
        {
            self.suggestions.push(Suggestion {
                icon: Icon::Globe,
                text: u.to_string(),
                detail: String::new(),
                hint: "Abrir",
                group: "",
                action: Action::Open(u),
            });
        }
        self.suggestions.push(Suggestion {
            icon: Icon::Search,
            text: text.clone(),
            detail: "DuckDuckGo".into(),
            hint: "Pesquisar",
            group: "",
            action: Action::Open(search_url(&text)),
        });
        let q = text.to_lowercase();
        for (i, t) in self.tabs.iter().enumerate() {
            if i != self.active
                && (t.title().to_lowercase().contains(&q) || t.url().as_str().to_lowercase().contains(&q))
                && self.suggestions.len() < 8
            {
                self.suggestions.push(Suggestion {
                    icon: t.icon(),
                    text: t.title(),
                    detail: host_of(t.url()),
                    hint: "Ir para a aba",
                    group: "Abas abertas",
                    action: Action::Tab(i),
                });
            }
        }
        for (u, title) in &self.history {
            if self.suggestions.len() >= 8 {
                break;
            }
            if u.as_str().to_lowercase().contains(&q) || title.to_lowercase().contains(&q) {
                self.suggestions.push(Suggestion {
                    icon: Icon::Clock,
                    text: if title.is_empty() { u.to_string() } else { title.clone() },
                    detail: host_of(u),
                    hint: "",
                    group: "Histórico",
                    action: Action::Open(u.clone()),
                });
            }
        }
    }

    fn run_suggestion(&mut self, i: usize) {
        match self.suggestions.get(i).map(|s| &s.action) {
            Some(Action::Tab(t)) => {
                let t = *t;
                self.activate(t);
            }
            Some(Action::Open(u)) => {
                let u = u.clone();
                self.close_palette();
                self.navigate(self.active, u);
            }
            None => {}
        }
    }

    fn submit_palette(&mut self) {
        // Enter sem editar abre o endereço que já está no campo
        if !self.palette_edited || self.suggestions.is_empty() {
            if let Some(u) = resolve_input(&self.omni.text) {
                self.close_palette();
                self.navigate(self.active, u);
            } else if !self.suggestions.is_empty() {
                self.run_suggestion(self.suggestion);
            }
            return;
        }
        self.run_suggestion(self.suggestion);
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()
    }

    // ------------------------------------------------------------- rolagem

    fn scroll_to(&mut self, y: f32) {
        let i = self.active;
        let max = self.max_scroll(i);
        let tab = &mut self.tabs[i];
        tab.scroll = y.clamp(0.0, max);
        tab.fragment = None;
        self.request_redraw();
    }

    fn scroll_by(&mut self, dy: f32) {
        let cur = self.effective_scroll(self.active);
        self.scroll_to(cur + dy);
        let (x, y) = self.mouse;
        self.update_hover(x, y);
    }

    fn drag_scrollbar(&mut self, thumb_top: f32) {
        let Some((track, th)) = self.scrollbar else { return };
        let free = (track.h - th).max(1.0);
        let max = self.max_scroll(self.active);
        self.scroll_to((thumb_top - track.y) / free * max);
    }

    fn set_zoom(&mut self, z: f32) {
        self.zoom = ((z * 100.0).round() / 100.0).clamp(0.3, 3.0);
        self.sync_viewport();
        self.request_redraw();
    }

    // ---------------------------------------------------------- mouse

    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h)
    }

    /// Ponto da janela em coordenadas do documento (px CSS), se cair na página.
    fn page_point(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let c = self.geo().content;
        if self.palette_open || !c.contains(x, y) {
            return None;
        }
        let tab = self.tab();
        if tab.kind() != Kind::Web || tab.error.is_some() {
            return None;
        }
        tab.frame.as_ref()?;
        let px = (x - c.x) / self.zoom;
        let py = (y - c.y) / self.zoom + self.effective_scroll(self.active);
        Some((px, py))
    }

    fn link_at(&self, x: f32, y: f32) -> Option<String> {
        let (px, py) = self.page_point(x, y)?;
        let frame = self.tab().frame.as_ref()?;
        // os links fixos (cabeçalhos) estão por cima e não rolam
        let sy = py - self.effective_scroll(self.active);
        if let Some((_, h)) = frame.fixed_links.iter().rev().find(|(r, _)| r.contains(px, sy)) {
            return Some(h.clone());
        }
        frame
            .links
            .iter()
            .rev()
            .find(|(r, _)| r.contains(px, py))
            .map(|(_, h)| h.clone())
    }

    fn update_hover(&mut self, x: f32, y: f32) {
        let hit = self.hit_at(x, y);
        let link = if hit.is_none() { self.link_at(x, y) } else { None };
        if hit != self.hover || link != self.hover_link {
            self.hover = hit;
            self.hover_link = link;
            self.request_redraw();
        }
        // a página fica sabendo onde o mouse está (hover, mousemove)
        let point = if hit.is_none() { self.page_point(x, y) } else { None };
        self.send_mouse(point);
        let mut visible = true;
        let cursor = if self.hover_link.is_some() {
            CursorIcon::Pointer
        } else if point.is_some() {
            use crate::style::Cursor as C;
            match self.tab().cursor {
                C::Auto | C::Default => CursorIcon::Default,
                C::Pointer => CursorIcon::Pointer,
                C::Text => CursorIcon::Text,
                C::Move => CursorIcon::Move,
                C::NotAllowed => CursorIcon::NotAllowed,
                C::Grab => CursorIcon::Grab,
                C::Grabbing => CursorIcon::Grabbing,
                C::Wait => CursorIcon::Wait,
                C::Progress => CursorIcon::Progress,
                C::Help => CursorIcon::Help,
                C::Crosshair => CursorIcon::Crosshair,
                C::EwResize => CursorIcon::EwResize,
                C::NsResize => CursorIcon::NsResize,
                C::NeswResize => CursorIcon::NeswResize,
                C::NwseResize => CursorIcon::NwseResize,
                C::ZoomIn => CursorIcon::ZoomIn,
                C::ZoomOut => CursorIcon::ZoomOut,
                C::None => {
                    visible = false;
                    CursorIcon::Default
                }
            }
        } else {
            match hit {
                Some(Hit::PaletteInput) => CursorIcon::Text,
                None | Some(Hit::ScrollThumb | Hit::ScrollTrack | Hit::Scrim) => CursorIcon::Default,
                Some(_) => CursorIcon::Pointer,
            }
        };
        if cursor != self.cursor || visible != self.cursor_visible {
            self.cursor = cursor;
            self.cursor_visible = visible;
            if let Some(w) = &self.window {
                w.set_cursor(cursor);
                w.set_cursor_visible(visible);
            }
        }
    }

    /// Avisa o motor da aba ativa da posição do mouse na página (só se mudou).
    fn send_mouse(&mut self, point: Option<(f32, f32)>) {
        let tab = &mut self.tabs[self.active];
        let same = match (tab.sent_mouse, point) {
            (Some(a), Some(b)) => (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5,
            (None, None) => true,
            _ => false,
        };
        if !same {
            tab.sent_mouse = point;
            tab.engine.mouse_move(tab.nav, point);
        }
    }

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        if let Some(off) = self.drag_scroll {
            self.drag_scrollbar(y - off);
            return;
        }
        // arrastando sobre a página: seleção de texto
        if self.tools.selecting
            && let Some((sx, sy)) = self.tools.press
            && ((x - sx).abs() > 3.0 || (y - sy).abs() > 3.0)
        {
            let c = self.geo().content;
            let (cx, cy) = (x.clamp(c.x, c.x + c.w - 1.0), y.clamp(c.y, c.y + c.h - 1.0));
            if let Some((px, py)) = self.page_point(cx, cy)
                && let Some(caret) = self.tools.caret_at(&self.fonts, px, py)
                && let Some((a, _)) = self.tools.sel
            {
                self.tools.sel = Some((a, caret));
                self.request_redraw();
            }
        }
        self.update_hover(x, y);
    }

    fn on_click(&mut self, el: &ActiveEventLoop) {
        let (x, y) = self.mouse;
        if self.menu.is_some() {
            match self.hit_at(x, y) {
                Some(Hit::MenuItem(i)) => self.run_menu(i),
                Some(Hit::Menu) => {}
                _ => {
                    self.menu = None;
                    self.request_redraw();
                }
            }
            return;
        }
        if self.select.is_some() {
            match self.hit_at(x, y) {
                Some(Hit::SelectOption(i)) => self.choose_option(i),
                Some(Hit::SelectList) => {}
                _ => {
                    self.select = None;
                    self.request_redraw();
                }
            }
            return;
        }
        match self.hit_at(x, y) {
            Some(Hit::Tab(i)) => self.activate(i),
            Some(Hit::CloseTab(i)) => self.close_tab(i, el),
            Some(Hit::NewTab) => self.new_tab(None, true),
            Some(Hit::Back) => self.go(-1),
            Some(Hit::Forward) => self.go(1),
            Some(Hit::Reload) => {
                if self.tab().loading {
                    self.stop()
                } else {
                    self.reload()
                }
            }
            Some(Hit::Settings | Hit::Customize) => self.open_internal(SETTINGS_URL),
            Some(Hit::Home) => self.go_home(),
            Some(Hit::Search | Hit::FakeBox) => self.open_palette(false),
            Some(Hit::TogglePanel) => self.toggle_panel(),
            Some(Hit::Capsule) => self.open_palette(true),
            Some(Hit::PaletteInput) => {
                let input_x = chrome::palette_text_x(self.logical_size().0);
                let fonts = &self.fonts;
                let scale = self.scale;
                let font = fonts.pick(crate::style::FontFamily::System, 400, false);
                let extend = self.mods.shift_key();
                self.omni.click(
                    x - input_x,
                    |s| fonts.measure(font, chrome::PALETTE_FONT * scale, s) / scale,
                    extend,
                );
                self.request_redraw();
            }
            Some(Hit::PaletteRow(i)) => {
                self.suggestion = i;
                self.run_suggestion(i);
            }
            Some(Hit::Scrim) => self.close_palette(),
            Some(Hit::Shortcut(i)) => {
                if let Ok(u) = Url::parse(pages::SHORTCUTS[i].url) {
                    self.navigate(self.active, u);
                }
            }
            Some(Hit::SettingsBack) => {
                if self.tab().index > 0 {
                    self.go(-1);
                } else {
                    self.go_home();
                }
            }
            Some(Hit::Section(sec)) => {
                let mut u = Url::parse(SETTINGS_URL).unwrap();
                match sec {
                    Section::About => u.set_fragment(Some("sobre")),
                    Section::Layout => u.set_fragment(Some("layout")),
                    Section::Appearance => {}
                }
                let tab = &mut self.tabs[self.active];
                tab.history[tab.index].url = u;
                self.request_redraw();
            }
            Some(Hit::Theme(i)) => {
                self.theme = i;
                save_config(self.theme, self.layout);
                self.request_redraw();
            }
            Some(Hit::Layout(mode)) => self.set_layout(mode),
            Some(Hit::Retry) => self.reload(),
            Some(Hit::FindClose) => self.close_find(),
            Some(Hit::FindBar | Hit::SelectList | Hit::Menu) => {}
            Some(Hit::SelectOption(i)) => self.choose_option(i),
            Some(Hit::MenuItem(i)) => self.run_menu(i),
            Some(Hit::ScrollThumb) => {
                if let Some((r, _)) = self.hits.iter().find(|(_, h)| *h == Hit::ScrollThumb) {
                    self.drag_scroll = Some(y - r.y);
                }
            }
            Some(Hit::ScrollTrack) => {
                if let Some((_, th)) = self.scrollbar {
                    self.drag_scroll = Some(th / 2.0);
                    self.drag_scrollbar(y - th / 2.0);
                }
            }
            None => {
                if self.mods.control_key() {
                    // Ctrl+clique: link em nova aba, sem passar pela página
                    if let Some(href) = self.link_at(x, y) {
                        self.open_link(&href, true);
                    }
                } else if let Some((px, py)) = self.page_point(x, y) {
                    // a página decide (eventos JS); se for link, o motor pede a navegação
                    self.sync_scroll();
                    let tab = self.tab();
                    tab.engine.click(tab.nav, px, py);
                    // e começa uma seleção de texto (vale se o mouse arrastar)
                    if let Some(frame) = self.tabs[self.active].frame.as_ref() {
                        self.tools.sync(&frame.items);
                    }
                    let had = self.tools.sel.is_some();
                    let caret = self.tools.caret_at(&self.fonts, px, py);
                    self.tools.sel = caret.map(|c| (c, c));
                    self.tools.selecting = true;
                    self.tools.press = Some((x, y));
                    if had {
                        self.request_redraw();
                    }
                }
            }
        }
    }

    /// Avisa o motor da rolagem atual (window.scrollY, getBoundingClientRect).
    fn sync_scroll(&mut self) {
        let i = self.active;
        if self.tabs.is_empty() {
            return;
        }
        let y = self.effective_scroll(i);
        let tab = &mut self.tabs[i];
        if (tab.sent_scroll - y).abs() > 0.5 && tab.frame.is_some() {
            tab.sent_scroll = y;
            tab.engine.scroll(tab.nav, y);
        }
    }

    fn on_mouse_button(&mut self, state: ElementState, button: MouseButton, el: &ActiveEventLoop) {
        match (state, button) {
            (ElementState::Pressed, MouseButton::Left) => self.on_click(el),
            (ElementState::Released, MouseButton::Left) => {
                self.drag_scroll = None;
                self.tools.selecting = false;
            }
            (ElementState::Pressed, MouseButton::Middle) => {
                let (x, y) = self.mouse;
                match self.hit_at(x, y) {
                    Some(Hit::Tab(i) | Hit::CloseTab(i)) => self.close_tab(i, el),
                    _ => {
                        if let Some(href) = self.link_at(x, y) {
                            self.open_link(&href, true);
                        }
                    }
                }
            }
            (ElementState::Pressed, MouseButton::Right) => self.open_menu(),
            (ElementState::Pressed, MouseButton::Back) => self.go(-1),
            (ElementState::Pressed, MouseButton::Forward) => self.go(1),
            _ => {}
        }
    }

    fn on_wheel(&mut self, delta: MouseScrollDelta) {
        let (x, y) = self.mouse;
        if let Some((_, popup, _, first)) = &mut self.select {
            let dy = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y,
                MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / 28.0,
            };
            let max = popup.labels.len().saturating_sub(10);
            *first = ((*first as f32 + dy.signum() * dy.abs().ceil()).max(0.0) as usize).min(max);
            self.request_redraw();
            return;
        }
        if self.palette_open || !self.geo().content.contains(x, y) {
            return;
        }
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (-x * 100.0, -y * 100.0),
            MouseScrollDelta::PixelDelta(p) => (-(p.x as f32) / self.scale, -(p.y as f32) / self.scale),
        };
        // Shift + roda: rolagem horizontal
        let (dx, dy) = if self.mods.shift_key() && dx == 0.0 { (dy, 0.0) } else { (dx, dy) };
        // um elemento que rola por dentro, sob o ponteiro, que ainda pode rolar nessa direção
        if let Some((px, py)) = self.page_point(x, y) {
            let tab = self.tab();
            let scroll = self.effective_scroll(self.active);
            if let Some(frame) = &tab.frame
                && let Some(sc) = frame.scrollers.iter().find(|s| {
                    let py = if s.fixed { py - scroll } else { py };
                    px >= s.rect.x
                        && px < s.rect.x + s.rect.w
                        && py >= s.rect.y
                        && py < s.rect.y + s.rect.h
                        && ((dy > 0.0 && s.y < s.max_y - 0.5)
                            || (dy < 0.0 && s.y > 0.5)
                            || (dx > 0.0 && s.x < s.max_x - 0.5)
                            || (dx < 0.0 && s.x > 0.5))
                })
            {
                tab.engine.scroll_element(tab.nav, sc.node, dx / self.zoom, dy / self.zoom);
                return;
            }
        }
        self.scroll_by(dy / self.zoom);
    }

    // ------------------------------------------------------------ teclado

    fn on_key(&mut self, ev: &KeyEvent, el: &ActiveEventLoop) {
        let ctrl = self.mods.control_key();
        let shift = self.mods.shift_key();
        let alt = self.mods.alt_key();

        if ctrl && !alt {
            if let Key::Character(c) = &ev.logical_key {
                match c.to_lowercase().as_str() {
                    "t" => return self.new_tab(None, true),
                    "w" => return self.close_tab(self.active, el),
                    "l" => return self.open_palette(true),
                    "k" => return self.open_palette(false),
                    "b" => return self.toggle_panel(),
                    "r" => return self.reload(),
                    "f" => return self.open_find(),
                    "=" | "+" => return self.set_zoom(self.zoom * 1.1),
                    "-" => return self.set_zoom(self.zoom / 1.1),
                    "0" => return self.set_zoom(1.0),
                    d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() && d != "0" => {
                        let n = (d.as_bytes()[0] - b'0') as usize;
                        let i = if n == 9 { self.tabs.len() - 1 } else { n - 1 };
                        return self.activate(i);
                    }
                    _ => {}
                }
            }
            if ev.logical_key == Key::Named(NamedKey::Tab) {
                let n = self.tabs.len();
                let i = if shift { (self.active + n - 1) % n } else { (self.active + 1) % n };
                return self.activate(i);
            }
        }
        match &ev.logical_key {
            Key::Named(NamedKey::F11) => return self.toggle_fullscreen(FullscreenBy::User),
            Key::Named(NamedKey::Escape) if self.fullscreen.is_some() && !self.palette_open => {
                return self.set_fullscreen(None);
            }
            Key::Named(NamedKey::F3) => return self.find_step(shift),
            Key::Named(NamedKey::F5) => return self.reload(),
            Key::Named(NamedKey::F6) => return self.open_palette(true),
            Key::Named(NamedKey::ArrowLeft) if alt => return self.go(-1),
            Key::Named(NamedKey::ArrowRight) if alt => return self.go(1),
            Key::Character(c) if alt && c.eq_ignore_ascii_case("d") => return self.open_palette(true),
            _ => {}
        }

        if self.menu.is_some() {
            if ev.logical_key == Key::Named(NamedKey::Escape) {
                self.menu = None;
                self.request_redraw();
            }
        } else if self.select.is_some() {
            self.select_key(ev);
        } else if self.palette_open {
            self.palette_key(ev, ctrl, shift, alt);
        } else if self.tools.find_open {
            self.find_key(ev, ctrl, shift, alt);
        } else {
            self.page_key(ev, ctrl, shift, alt);
        }
    }

    /// Teclas com a lista de um `<select>` aberta.
    fn select_key(&mut self, ev: &KeyEvent) {
        let Some((_, popup, hl, _)) = &mut self.select else { return };
        let n = popup.labels.len();
        let ok = |i: usize| !popup.disabled.get(i).copied().unwrap_or(false);
        match &ev.logical_key {
            Key::Named(NamedKey::ArrowDown) => {
                let mut i = *hl;
                while i + 1 < n {
                    i += 1;
                    if ok(i) {
                        *hl = i;
                        break;
                    }
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                let mut i = *hl;
                while i > 0 {
                    i -= 1;
                    if ok(i) {
                        *hl = i;
                        break;
                    }
                }
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                let i = *hl;
                return self.choose_option(i);
            }
            Key::Named(NamedKey::Escape | NamedKey::Tab) => {
                self.select = None;
            }
            _ => {}
        }
        if let Some((_, _, hl, first)) = &mut self.select {
            const ROWS: usize = 10;
            if *hl < *first {
                *first = *hl;
            } else if *hl >= *first + ROWS {
                *first = *hl + 1 - ROWS;
            }
        }
        self.request_redraw();
    }

    /// Abre o menu de contexto onde está o mouse.
    fn open_menu(&mut self) {
        let (x, y) = self.mouse;
        self.select = None;
        let mut items: Vec<(&'static str, Option<MenuAction>)> = Vec::new();
        if self.page_point(x, y).is_some() {
            if let Some(href) = self.link_at(x, y) {
                let abs = self
                    .tab()
                    .frame
                    .as_ref()
                    .and_then(|f| f.base.join(href.trim()).ok())
                    .map(|u| u.to_string())
                    .unwrap_or(href);
                items.push(("Abrir link em nova aba", Some(MenuAction::OpenLinkTab(abs.clone()))));
                items.push(("Copiar endereço do link", Some(MenuAction::CopyLink(abs))));
                items.push(("", None));
            }
            if self.tools.ordered().is_some() {
                items.push(("Copiar", Some(MenuAction::CopySelection)));
            }
            items.push(("Selecionar tudo", Some(MenuAction::SelectAll)));
            items.push(("Buscar na página…", Some(MenuAction::Find)));
            items.push(("", None));
        }
        let tab = self.tab();
        if tab.index > 0 {
            items.push(("Voltar", Some(MenuAction::Back)));
        }
        if tab.index + 1 < tab.history.len() {
            items.push(("Avançar", Some(MenuAction::Forward)));
        }
        items.push(("Recarregar", Some(MenuAction::Reload)));
        items.push((if self.fullscreen.is_some() { "Sair da tela cheia" } else { "Tela cheia" }, Some(MenuAction::Fullscreen)));
        self.menu = Some(((x, y), items));
        self.request_redraw();
    }

    fn run_menu(&mut self, i: usize) {
        let Some((_, items)) = self.menu.take() else { return };
        let Some((_, Some(action))) = items.get(i).cloned() else { return };
        match action {
            MenuAction::OpenLinkTab(u) => self.open_link(&u, true),
            MenuAction::CopyLink(u) => {
                if let Some(cb) = self.clipboard() {
                    let _ = cb.set_text(u);
                }
            }
            MenuAction::CopySelection => {
                self.copy_selection();
            }
            MenuAction::SelectAll => {
                if let Some(frame) = self.tabs[self.active].frame.as_ref() {
                    self.tools.sync(&frame.items);
                }
                self.tools.select_all();
            }
            MenuAction::Back => self.go(-1),
            MenuAction::Forward => self.go(1),
            MenuAction::Reload => self.reload(),
            MenuAction::Find => self.open_find(),
            MenuAction::Fullscreen => self.toggle_fullscreen(FullscreenBy::User),
        }
        self.request_redraw();
    }

    /// Escolhe a opção `i` da lista aberta.
    fn choose_option(&mut self, i: usize) {
        if let Some((nav, popup, _, _)) = self.select.take()
            && !popup.disabled.get(i).copied().unwrap_or(false)
        {
            let tab = self.tab();
            if tab.nav == nav {
                tab.engine.select_choose(nav, popup.node, i);
            }
        }
        self.request_redraw();
    }

    /// Teclas com a busca na página aberta.
    fn find_key(&mut self, ev: &KeyEvent, ctrl: bool, shift: bool, alt: bool) {
        let f = &mut self.tools.find;
        let mut changed = false;
        match &ev.logical_key {
            Key::Named(NamedKey::Enter) => return self.find_step(shift),
            Key::Named(NamedKey::Escape) => return self.close_find(),
            Key::Named(NamedKey::Backspace) => {
                f.backspace(ctrl);
                changed = true;
            }
            Key::Named(NamedKey::Delete) => {
                f.delete(ctrl);
                changed = true;
            }
            Key::Named(NamedKey::ArrowLeft) => f.left(shift, ctrl),
            Key::Named(NamedKey::ArrowRight) => f.right(shift, ctrl),
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "a" => f.select_all(),
                "v" => {
                    if let Some(text) = self.clipboard().and_then(|cb| cb.get_text().ok()) {
                        let line = text.lines().next().unwrap_or("").to_string();
                        self.tools.find.insert(&line);
                        changed = true;
                    }
                }
                _ => {}
            },
            _ => {
                if !ctrl
                    && !alt
                    && let Some(t) = &ev.text
                    && !t.chars().any(char::is_control)
                {
                    f.insert(t);
                    changed = true;
                }
            }
        }
        if changed {
            self.tools.current = 0;
            self.refresh_find(true);
        }
        self.request_redraw();
    }

    /// Entra/sai da tela cheia (F11 alterna).
    fn toggle_fullscreen(&mut self, by: FullscreenBy) {
        if self.fullscreen.is_some() {
            self.set_fullscreen(None);
        } else {
            self.set_fullscreen(Some(by));
        }
    }

    fn set_fullscreen(&mut self, by: Option<FullscreenBy>) {
        let was = self.fullscreen;
        if was == by {
            return;
        }
        // saindo da tela cheia da página: avisa a página e volta a rolagem
        if was == Some(FullscreenBy::Page) && by.is_none() {
            let tab = self.tab();
            tab.engine.exit_fullscreen(tab.nav);
            let y = self.fs_scroll;
            self.fullscreen = None;
            self.scroll_to(y);
        }
        if by == Some(FullscreenBy::Page) {
            self.fs_scroll = self.effective_scroll(self.active);
            self.scroll_to(0.0);
        }
        self.fullscreen = by;
        if let Some(w) = &self.window {
            w.set_fullscreen(by.map(|_| winit::window::Fullscreen::Borderless(None)));
        }
        self.sync_viewport();
        self.request_redraw();
    }

