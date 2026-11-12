//! Pincel da interface: desenha em coordenadas lógicas (DIP) e converte
//! para pixels do dispositivo pela escala de DPI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::fonts::{FontId, Fonts};
use crate::layout::Rect;
use crate::paint::{Canvas, ImageData};
use crate::style::{Color, FontFamily};
use crate::svg::{self, Svg};

#[derive(Clone, Copy, PartialEq)]
pub enum Weight {
    Regular,
    Semibold,
    Bold,
}

/// Ícones da interface: Mage Icons (estilo "stroke"), embutidos no binário
/// e desenhados pelo renderizador de SVG do próprio Ferro.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Back,
    Forward,
    Reload,
    Close,
    Plus,
    Search,
    Lock,
    Unlocked,
    Globe,
    Settings,
    Info,
    Swatch,
    Clock,
    ArrowUpRight,
    ChevronRight,
    ChevronLeft,
    Warning,
    Play,
    File,
    Home,
    Panel,
    Download,
}

impl Icon {
