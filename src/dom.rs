//! DOM: a árvore do documento. Os nós ficam numa arena (um Vec) e se
//! referenciam por índice (NodeId), o que evita Rc/RefCell e ciclos.

pub type NodeId = usize;

/// DocumentFragment é um elemento com esta tag (nunca aparece na árvore).
pub const FRAGMENT_TAG: &str = "#document-fragment";

/// Elementos sem conteúdo nem tag de fechamento.
pub const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr",
];

/// Elementos cujo texto não é escapado ao serializar.
const RAW_TEXT: &[&str] = &["script", "style", "xmp", "iframe", "noembed", "noframes", "plaintext", "noscript"];

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('\u{a0}', "&nbsp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('\u{a0}', "&nbsp;").replace('"', "&quot;")
}

#[derive(Debug, Clone)]
pub enum NodeData {
    Document,
    Doctype(String),
    Element(ElementData),
    Text(String),
    Comment(String),
}

