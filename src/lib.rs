//! Ferro: um motor de navegador escrito do zero em Rust.
//!
//! Pipeline: rede → HTML → DOM → CSS → cascata → caixas → layout → pintura.

pub mod alloc;
pub mod canvas;
pub mod cookies;
pub mod cors;
pub mod css;
pub mod dados;
pub mod dom;
pub mod engine;
pub mod fonts;
pub mod html;
pub mod httpcache;
pub mod internal;
pub mod jshost;
pub mod layout;
pub mod media;
pub mod net;
pub mod paint;
pub mod script;
pub mod style;
pub mod svg;
pub mod ui;

/// Folha de estilo padrão do motor.
pub const UA_STYLESHEET: &str = include_str!("ua.css");
