//! Pintura em software: executa a lista de exibição (retângulos, cantos
//! arredondados, texto, imagens, recortes) num buffer de pixels 0x00RRGGBB.
//! O mesmo Canvas pinta as páginas e a interface do navegador.

use std::path::Path;
use std::sync::Arc;

use crate::fonts::{FontId, Fonts};
use crate::layout::Rect;
use crate::style::Color;

/// Imagem decodificada (RGBA, alfa não pré-multiplicado).
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageData({}x{})", self.width, self.height)
    }
}

