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

    pub fn new() -> Self {
        Document {
            nodes: vec![Node {
                parent: None,
                children: Vec::new(),
                data: NodeData::Document,
            }],
            shadow_roots: Default::default(),
            shadow_hosts: Default::default(),
        }
    }

    pub fn create(&mut self, data: NodeData) -> NodeId {
        self.nodes.push(Node {
            parent: None,
            children: Vec::new(),
            data,
        });
        self.nodes.len() - 1
    }

    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn element(&self, id: NodeId) -> Option<&ElementData> {
        match &self.nodes[id].data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }

    pub fn tag(&self, id: NodeId) -> Option<&str> {
        self.element(id).map(|e| e.tag.as_str())
    }

    pub fn parent_element(&self, id: NodeId) -> Option<NodeId> {
        let p = self.nodes[id].parent?;
        self.element(p).map(|_| p)
    }

    /// Irmãos que são elementos, na ordem do documento (inclui o próprio nó).
    pub fn element_siblings(&self, id: NodeId) -> Vec<NodeId> {
        match self.nodes[id].parent {
            Some(p) => self.nodes[p]
                .children
                .iter()
                .copied()
                .filter(|&c| self.element(c).is_some())
                .collect(),
            None => vec![id],
        }
    }

