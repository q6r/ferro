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

