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

