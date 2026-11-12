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

