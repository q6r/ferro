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

    /// Todos os descendentes em pré-ordem (ordem do documento).
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.nodes[id].children.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend(self.nodes[n].children.iter().rev().copied());
        }
        out
    }

    pub fn find_all(&self, tag: &str) -> Vec<NodeId> {
        self.descendants(Self::ROOT)
            .into_iter()
            .filter(|&n| self.tag(n) == Some(tag))
            .collect()
    }

    pub fn find_first(&self, tag: &str) -> Option<NodeId> {
        self.descendants(Self::ROOT)
            .into_iter()
            .find(|&n| self.tag(n) == Some(tag))
    }

    pub fn text_content(&self, id: NodeId) -> String {
        let mut s = String::new();
        if let NodeData::Text(t) = &self.nodes[id].data {
            s.push_str(t);
        }
        for d in self.descendants(id) {
            if let NodeData::Text(t) = &self.nodes[d].data {
                s.push_str(t);
            }
        }
        s
    }

    // ------------------------------------------------------------ mutação
    // (usada pelos scripts da página; os nós nunca saem da arena, só da árvore)

    /// Está ligado ao documento? (A sombra conta pelo hospedeiro.)
    pub fn is_connected(&self, mut id: NodeId) -> bool {
        loop {
            if id == Self::ROOT {
                return true;
            }
            match self.nodes[id].parent {
                Some(p) => id = p,
                None => match self.shadow_hosts.get(&id) {
                    Some(&h) => id = h,
                    None => return false,
                },
            }
        }
    }

    /// Cria a raiz de sombra de `host` (attachShadow).
    pub fn attach_shadow(&mut self, host: NodeId) -> NodeId {
        let root = self.create_fragment();
        self.shadow_roots.insert(host, root);
        self.shadow_hosts.insert(root, host);
        root
    }

    /// Filhos na "árvore achatada", a que é desenhada: um hospedeiro mostra a
    /// sombra dele, e cada `<slot>` mostra os filhos do hospedeiro destinados a ele.
    pub fn flat_children(&self, n: NodeId) -> std::borrow::Cow<'_, [NodeId]> {
        use std::borrow::Cow;
        if self.shadow_roots.is_empty() {
            return Cow::Borrowed(&self.nodes[n].children);
        }
        if let Some(&root) = self.shadow_roots.get(&n) {
            return Cow::Borrowed(&self.nodes[root].children);
        }
        if self.tag(n) == Some("slot")
            && let Some(host) = self.slot_host(n)
        {
            let assigned = self.assigned_nodes(host, n);
            if !assigned.is_empty() {
                return Cow::Owned(assigned);
            }
        }
        Cow::Borrowed(&self.nodes[n].children)
    }

    /// Pai na árvore achatada: o `<slot>` para quem foi distribuído, o
    /// hospedeiro para a raiz de sombra.
    pub fn flat_parent(&self, n: NodeId) -> Option<NodeId> {
        let Some(p) = self.nodes[n].parent else {
            return self.shadow_hosts.get(&n).copied();
        };
        if let Some(&root) = self.shadow_roots.get(&p) {
            let name = match &self.nodes[n].data {
                NodeData::Element(e) => e.attr("slot").unwrap_or(""),
                NodeData::Text(_) => "",
                _ => return Some(p),
            };
            let slot = self.descendants(root).into_iter().find(|&d| {
                self.element(d)
                    .is_some_and(|e| e.tag == "slot" && e.attr("name").unwrap_or("") == name)
            });
            return Some(slot.unwrap_or(p));
        }
        Some(p)
    }

    /// Hospedeiro da sombra onde está o `<slot>`.
    pub fn slot_host(&self, slot: NodeId) -> Option<NodeId> {
        let mut r = slot;
        while let Some(p) = self.nodes[r].parent {
            r = p;
        }
        self.shadow_hosts.get(&r).copied()
    }

    /// Filhos do hospedeiro que vão para esse `<slot>` (pelo atributo `slot`).
    pub fn assigned_nodes(&self, host: NodeId, slot: NodeId) -> Vec<NodeId> {
        let name = self.element(slot).and_then(|e| e.attr("name")).unwrap_or("");
        self.nodes[host]
            .children
            .iter()
            .copied()
            .filter(|&c| match &self.nodes[c].data {
                NodeData::Element(e) => e.attr("slot").unwrap_or("") == name,
                NodeData::Text(_) => name.is_empty(),
                _ => false,
            })
            .collect()
    }

    /// Tira o nó do pai (ele continua existindo, solto).
    pub fn detach(&mut self, id: NodeId) {
        if let Some(p) = self.nodes[id].parent.take() {
            self.nodes[p].children.retain(|&c| c != id);
        }
    }

    /// `ancestor` é o próprio nó ou um ancestral de `id`?
    pub fn is_inclusive_ancestor(&self, ancestor: NodeId, mut id: NodeId) -> bool {
        loop {
            if id == ancestor {
                return true;
            }
            match self.nodes[id].parent {
                Some(p) => id = p,
                None => return false,
            }
        }
    }

    /// Insere `child` em `parent` antes de `before` (ou no fim). Fragmentos
    /// são "desembrulhados": entram os filhos deles.
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: Option<NodeId>) {
        if self.is_fragment(child) {
            let kids = std::mem::take(&mut self.nodes[child].children);
            for k in kids {
                self.nodes[k].parent = None;
                self.insert_before(parent, k, before);
            }
            return;
        }
        self.detach(child);
        let idx = before
            .and_then(|b| self.nodes[parent].children.iter().position(|&c| c == b))
            .unwrap_or(self.nodes[parent].children.len());
        self.nodes[parent].children.insert(idx, child);
        self.nodes[child].parent = Some(parent);
    }

    pub fn is_fragment(&self, id: NodeId) -> bool {
        self.tag(id) == Some(FRAGMENT_TAG)
    }

    pub fn create_element(&mut self, tag: &str) -> NodeId {
        self.create(NodeData::Element(ElementData {
            tag: tag.to_string(),
            attrs: Vec::new(),
        }))
    }

    pub fn create_fragment(&mut self) -> NodeId {
        self.create_element(FRAGMENT_TAG)
    }

    pub fn set_attr(&mut self, id: NodeId, name: &str, value: &str) {
        if let NodeData::Element(e) = &mut self.nodes[id].data {
            match e.attrs.iter_mut().find(|(k, _)| k == name) {
                Some((_, v)) => *v = value.to_string(),
                None => e.attrs.push((name.to_string(), value.to_string())),
            }
        }
    }

    pub fn remove_attr(&mut self, id: NodeId, name: &str) -> bool {
        if let NodeData::Element(e) = &mut self.nodes[id].data {
            let before = e.attrs.len();
            e.attrs.retain(|(k, _)| k != name);
            return e.attrs.len() != before;
        }
        false
    }

    /// Texto de um nó de texto/comentário.
    pub fn set_data(&mut self, id: NodeId, text: &str) {
        match &mut self.nodes[id].data {
            NodeData::Text(t) | NodeData::Comment(t) => *t = text.to_string(),
            _ => {}
        }
    }

    /// `textContent = ...`: troca todos os filhos por um texto.
    pub fn set_text_content(&mut self, id: NodeId, text: &str) {
        if matches!(self.nodes[id].data, NodeData::Text(_) | NodeData::Comment(_)) {
            self.set_data(id, text);
            return;
        }
        self.remove_children(id);
        if !text.is_empty() {
            let t = self.create(NodeData::Text(text.to_string()));
            self.append(id, t);
        }
    }

    pub fn remove_children(&mut self, id: NodeId) {
        let kids = std::mem::take(&mut self.nodes[id].children);
        for k in kids {
            self.nodes[k].parent = None;
        }
    }

