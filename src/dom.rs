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

#[derive(Debug, Clone)]
pub struct ElementData {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
}

impl ElementData {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn id(&self) -> Option<&str> {
        self.attr("id")
    }

    pub fn classes(&self) -> impl Iterator<Item = &str> {
        self.attr("class").unwrap_or("").split_ascii_whitespace()
    }

    pub fn has_class(&self, class: &str) -> bool {
        self.classes().any(|c| c == class)
    }
}

#[derive(Debug)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub data: NodeData,
}

#[derive(Debug)]
pub struct Document {
    pub nodes: Vec<Node>,
    /// Shadow DOM: hospedeiro → raiz da sombra (um fragmento).
    pub shadow_roots: std::collections::HashMap<NodeId, NodeId>,
    /// Raiz da sombra → hospedeiro.
    pub shadow_hosts: std::collections::HashMap<NodeId, NodeId>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub const ROOT: NodeId = 0;

