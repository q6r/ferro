//! Parser HTML: tokenizador + construtor de árvore.
//!
//! Segue a ideia do algoritmo WHATWG de forma simplificada: trata os casos
//! que aparecem de verdade na web (tags sem fechamento, <p> e <li> fechados
//! implicitamente, <script>/<style> como texto bruto, entidades), mas ainda
//! não implementa todos os modos de inserção (tabelas, adoption agency).

use crate::dom::{Document, ElementData, NodeData, NodeId};

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Doctype(String),
    StartTag {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    EndTag {
        name: String,
    },
    Text(String),
    Comment(String),
}

// ---------------------------------------------------------------- tokenizador

/// Scripts ligados? Com eles, `<noscript>` vira texto bruto (e não aparece).
static SCRIPTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_scripting(on: bool) {
    SCRIPTING.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn scripting() -> bool {
    SCRIPTING.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn tokenize(input: &str) -> Vec<Token> {
    let scripting = scripting();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut i = 0;

    fn flush(text: &mut String, tokens: &mut Vec<Token>) {
        if !text.is_empty() {
            tokens.push(Token::Text(decode_entities(text)));
            text.clear();
        }
    }

    while i < len {
        let c = chars[i];
        if c != '<' {
            text.push(c);
            i += 1;
            continue;
        }
        match chars.get(i + 1).copied() {
            Some('!') => {
                flush(&mut text, &mut tokens);
                if starts_with(&chars, i + 2, "--") {
                    let (end, after) = match find(&chars, i + 4, "-->") {
                        Some(e) => (e, e + 3),
                        None => (len, len),
                    };
                    tokens.push(Token::Comment(chars[i + 4..end].iter().collect()));
                    i = after;
                } else {
                    let end = find_char(&chars, i + 2, '>').unwrap_or(len);
                    let content: String = chars[i + 2..end].iter().collect();
                    if content.to_ascii_lowercase().starts_with("doctype") {
                        tokens.push(Token::Doctype(content[7..].trim().to_string()));
                    } else {
                        tokens.push(Token::Comment(content));
                    }
                    i = (end + 1).min(len);
                }
            }
            Some('?') => {
                // <?xml ...?> e afins viram comentário
                flush(&mut text, &mut tokens);
                let end = find_char(&chars, i + 2, '>').unwrap_or(len);
                tokens.push(Token::Comment(chars[i + 2..end].iter().collect()));
                i = (end + 1).min(len);
            }
            Some('/') => match chars.get(i + 2).copied() {
                Some(c2) if c2.is_ascii_alphabetic() => {
                    flush(&mut text, &mut tokens);
                    let mut j = i + 2;
                    let mut name = String::new();
                    while j < len && !chars[j].is_whitespace() && chars[j] != '>' && chars[j] != '/' {
                        name.push(chars[j].to_ascii_lowercase());
                        j += 1;
                    }
                    let end = find_char(&chars, j, '>').unwrap_or(len);
                    tokens.push(Token::EndTag { name });
                    i = (end + 1).min(len);
                }
                Some('>') => i += 3,
                _ => {
                    flush(&mut text, &mut tokens);
                    let end = find_char(&chars, i + 2, '>').unwrap_or(len);
                    tokens.push(Token::Comment(chars[i + 2..end].iter().collect()));
                    i = (end + 1).min(len);
                }
            },
            Some(c2) if c2.is_ascii_alphabetic() => {
                flush(&mut text, &mut tokens);
                let (tok, j) = parse_start_tag(&chars, i + 1);
                i = j;
                let raw = match &tok {
                    Token::StartTag {
                        name,
                        self_closing: false,
                        ..
                    } => match name.as_str() {
                        "script" | "style" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext" => {
                            Some((name.clone(), false))
                        }
                        "noscript" if scripting => Some((name.clone(), false)),
                        "title" | "textarea" => Some((name.clone(), true)),
                        _ => None,
                    },
                    _ => None,
                };
                tokens.push(tok);
                // Elementos de texto bruto: tudo até </nome> é texto.
                if let Some((name, decode)) = raw {
                    let end = find_ci(&chars, i, &format!("</{name}")).unwrap_or(len);
                    let content: String = chars[i..end].iter().collect();
                    if !content.is_empty() {
                        tokens.push(Token::Text(if decode {
                            decode_entities(&content)
                        } else {
                            content
                        }));
                    }
                    tokens.push(Token::EndTag { name });
                    i = find_char(&chars, end, '>').map(|e| e + 1).unwrap_or(len);
                }
            }
            _ => {
                text.push('<');
                i += 1;
            }
        }
    }
    flush(&mut text, &mut tokens);
    tokens
}

fn parse_start_tag(chars: &[char], start: usize) -> (Token, usize) {
    let len = chars.len();
    let mut j = start;
    let mut name = String::new();
    while j < len && !chars[j].is_whitespace() && chars[j] != '>' && chars[j] != '/' {
        name.push(chars[j].to_ascii_lowercase());
        j += 1;
    }
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut self_closing = false;
    loop {
        while j < len && chars[j].is_whitespace() {
            j += 1;
        }
        if j >= len {
            break;
        }
        match chars[j] {
            '>' => {
                j += 1;
                break;
            }
            '/' => {
                if chars.get(j + 1) == Some(&'>') {
                    self_closing = true;
                    j += 2;
                    break;
                }
                j += 1;
                continue;
            }
            _ => {}
        }
        // nome do atributo (o primeiro caractere pode ser qualquer coisa, até '=')
        let mut attr_name = String::new();
        attr_name.push(chars[j].to_ascii_lowercase());
        j += 1;
        while j < len && !chars[j].is_whitespace() && !matches!(chars[j], '=' | '>' | '/') {
            attr_name.push(chars[j].to_ascii_lowercase());
            j += 1;
        }
        while j < len && chars[j].is_whitespace() {
            j += 1;
        }
        let mut value = String::new();
        if j < len && chars[j] == '=' {
            j += 1;
            while j < len && chars[j].is_whitespace() {
                j += 1;
            }
            if j < len && (chars[j] == '"' || chars[j] == '\'') {
                let q = chars[j];
                j += 1;
                while j < len && chars[j] != q {
                    value.push(chars[j]);
                    j += 1;
                }
                j += 1;
            } else {
                while j < len && !chars[j].is_whitespace() && chars[j] != '>' {
                    value.push(chars[j]);
                    j += 1;
                }
            }
            value = decode_entities(&value);
        }
        if !attrs.iter().any(|(k, _)| *k == attr_name) {
            attrs.push((attr_name, value));
        }
    }
    (
        Token::StartTag {
            name,
            attrs,
            self_closing,
        },
        j.min(len),
    )
}

fn starts_with(chars: &[char], at: usize, pat: &str) -> bool {
    let mut k = at;
    for p in pat.chars() {
        if chars.get(k) != Some(&p) {
            return false;
        }
        k += 1;
    }
    true
}

fn find(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    (from..chars.len()).find(|&k| starts_with(chars, k, pat))
}

fn find_char(chars: &[char], from: usize, c: char) -> Option<usize> {
    (from..chars.len()).find(|&k| chars[k] == c)
}

/// Busca sem diferenciar maiúsculas (para </SCRIPT>, </Style>...).
fn find_ci(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    let pat: Vec<char> = pat.chars().collect();
    (from..chars.len()).find(|&k| {
        pat.iter()
            .enumerate()
            .all(|(n, p)| chars.get(k + n).is_some_and(|c| c.to_ascii_lowercase() == *p))
    })
}

// ------------------------------------------------------------------ entidades

const ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"), ("lt", "<"), ("gt", ">"), ("quot", "\""), ("apos", "'"),
    ("nbsp", "\u{a0}"), ("copy", "©"), ("reg", "®"), ("trade", "™"),
    ("hellip", "…"), ("mdash", "—"), ("ndash", "–"), ("lsquo", "‘"),
    ("rsquo", "’"), ("ldquo", "“"), ("rdquo", "”"), ("laquo", "«"),
    ("raquo", "»"), ("middot", "·"), ("bull", "•"), ("euro", "€"),
    ("pound", "£"), ("yen", "¥"), ("cent", "¢"), ("deg", "°"), ("times", "×"),
    ("divide", "÷"), ("plusmn", "±"), ("para", "¶"), ("sect", "§"),
    ("shy", "\u{ad}"), ("iexcl", "¡"), ("iquest", "¿"), ("frac12", "½"),
    ("frac14", "¼"), ("frac34", "¾"), ("ordf", "ª"), ("ordm", "º"),
    ("aacute", "á"), ("agrave", "à"), ("acirc", "â"), ("atilde", "ã"),
    ("auml", "ä"), ("eacute", "é"), ("egrave", "è"), ("ecirc", "ê"),
    ("euml", "ë"), ("iacute", "í"), ("igrave", "ì"), ("icirc", "î"),
    ("oacute", "ó"), ("ograve", "ò"), ("ocirc", "ô"), ("otilde", "õ"),
    ("ouml", "ö"), ("uacute", "ú"), ("ugrave", "ù"), ("ucirc", "û"),
    ("uuml", "ü"), ("ccedil", "ç"), ("ntilde", "ñ"), ("szlig", "ß"),
    ("Aacute", "Á"), ("Agrave", "À"), ("Acirc", "Â"), ("Atilde", "Ã"),
    ("Eacute", "É"), ("Ecirc", "Ê"), ("Iacute", "Í"), ("Oacute", "Ó"),
    ("Ocirc", "Ô"), ("Otilde", "Õ"), ("Uacute", "Ú"), ("Ccedil", "Ç"),
    ("Ntilde", "Ñ"), ("rarr", "→"), ("larr", "←"), ("uarr", "↑"),
    ("darr", "↓"), ("harr", "↔"), ("hearts", "♥"), ("check", "✓"),
    ("zwj", "\u{200d}"), ("zwnj", "\u{200c}"), ("ensp", "\u{2002}"),
    ("emsp", "\u{2003}"), ("thinsp", "\u{2009}"),
];

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        if let Some((decoded, used)) = decode_one(rest) {
            out.push_str(&decoded);
            rest = &rest[used..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Decodifica uma referência começando em `s` (que começa com '&').
fn decode_one(s: &str) -> Option<(String, usize)> {
    let body = &s[1..];
    if let Some(num) = body.strip_prefix('#') {
        let (digits, radix, skip) = if let Some(hex) = num.strip_prefix(['x', 'X']) {
            let d: String = hex.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            (d, 16, 3)
        } else {
            let d: String = num.chars().take_while(|c| c.is_ascii_digit()).collect();
            (d, 10, 2)
        };
        if digits.is_empty() {
            return None;
        }
        let mut used = skip + digits.len();
        if s[used..].starts_with(';') {
            used += 1;
        }
        let cp = u32::from_str_radix(&digits, radix).unwrap_or(0xFFFD);
        let ch = char::from_u32(cp).filter(|&c| c != '\0').unwrap_or('\u{FFFD}');
        return Some((ch.to_string(), used));
    }
    let name: String = body.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if name.is_empty() || !body[name.len()..].starts_with(';') {
        return None;
    }
    ENTITIES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| (v.to_string(), 1 + name.len() + 1))
}

// -------------------------------------------------------- construtor de árvore

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
    "source", "track", "wbr", "keygen",
];

const HEAD_CONTENT: &[&str] = &["title", "meta", "link", "style", "script", "base", "template"];

/// Tags de início que fecham um <p> aberto.
const CLOSES_P: &[&str] = &[
    "address", "article", "aside", "blockquote", "center", "details", "dialog", "dir", "div",
    "dl", "fieldset", "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5",
    "h6", "header", "hgroup", "hr", "main", "menu", "nav", "ol", "p", "pre", "section",
    "summary", "table", "ul", "li", "dd", "dt", "listing",
];

/// Elementos que delimitam o "escopo" na busca por tags abertas.
const SCOPE_BOUNDARY: &[&str] = &[
    "html", "table", "td", "th", "caption", "marquee", "object", "applet", "template", "button",
];

/// Analisa `input` como o conteúdo de um elemento `<context>` (innerHTML) e
/// cria os nós (soltos) em `doc`. Devolve os nós de primeiro nível.
pub fn parse_fragment(input: &str, context: &str, doc: &mut Document) -> Vec<NodeId> {
    let (wrapped, path): (String, &[&str]) = match context {
        "table" => (format!("<table>{input}</table>"), &["table"]),
        "tbody" | "thead" | "tfoot" => (format!("<table><tbody>{input}</tbody></table>"), &["table", "tbody"]),
        "tr" => (format!("<table><tbody><tr>{input}</tr></tbody></table>"), &["table", "tbody", "tr"]),
        "select" => (format!("<select>{input}</select>"), &["select"]),
        "ul" | "ol" => (format!("<ul>{input}</ul>"), &["ul"]),
        _ => (format!("<body>{input}"), &[]),
    };
    let tmp = parse(&wrapped);
    let Some(mut container) = tmp.find_first("body") else { return Vec::new() };
    for tag in path {
        match tmp.node(container).children.iter().copied().find(|&c| tmp.tag(c) == Some(tag)) {
            Some(c) => container = c,
            None => break,
        }
    }
    tmp.node(container).children.iter().map(|&c| copy_into(&tmp, c, doc)).collect()
}

/// Copia a subárvore `id` de `src` para `dst` (a cópia fica solta).
pub fn copy_into(src: &Document, id: NodeId, dst: &mut Document) -> NodeId {
    let n = dst.create(src.node(id).data.clone());
    for &c in &src.node(id).children {
        let k = copy_into(src, c, dst);
        dst.append(n, k);
    }
    n
}

pub fn parse(input: &str) -> Document {
    let mut b = TreeBuilder {
        doc: Document::new(),
        stack: Vec::new(),
        html: None,
        head: None,
        body: None,
        skip_newline: false,
    };
    for t in tokenize(input) {
        b.process(t);
    }
    b.ensure_body();
    b.doc
}

struct TreeBuilder {
    doc: Document,
    stack: Vec<NodeId>,
    html: Option<NodeId>,
    head: Option<NodeId>,
    body: Option<NodeId>,
    /// A quebra de linha logo após <pre>/<textarea> é ignorada.
    skip_newline: bool,
}

impl TreeBuilder {
    fn current(&self) -> NodeId {
        self.stack.last().copied().unwrap_or(Document::ROOT)
    }

    fn tag_at(&self, idx: usize) -> &str {
        self.doc.tag(self.stack[idx]).unwrap_or("")
    }

    fn new_element(&mut self, name: &str, attrs: Vec<(String, String)>) -> NodeId {
        self.doc.create(NodeData::Element(ElementData {
            tag: name.to_string(),
            attrs,
        }))
    }

    /// Acrescenta ao elemento os atributos que ele ainda não tem (regra do
    /// HTML para `<html>` e `<body>` que aparecem depois de criados).
    fn merge_attrs(&mut self, n: NodeId, attrs: Vec<(String, String)>) {
        if let NodeData::Element(e) = &mut self.doc.nodes[n].data {
            for (k, v) in attrs {
                if !e.attrs.iter().any(|(a, _)| *a == k) {
                    e.attrs.push((k, v));
                }
            }
        }
    }

    fn ensure_html(&mut self) {
        if self.html.is_none() {
            let h = self.new_element("html", Vec::new());
            self.doc.append(Document::ROOT, h);
            self.stack.push(h);
            self.html = Some(h);
        }
    }

    fn ensure_head(&mut self) {
        self.ensure_html();
        if self.head.is_none() {
            let h = self.new_element("head", Vec::new());
            self.doc.append(self.html.unwrap(), h);
            self.stack.push(h);
            self.head = Some(h);
        }
    }

    fn ensure_body(&mut self) {
        self.ensure_head();
        if self.body.is_none() {
            // fecha o <head> (e o que estiver aberto dentro dele)
            if let Some(pos) = self.stack.iter().position(|&n| Some(n) == self.head) {
                self.stack.truncate(pos);
            }
            let b = self.new_element("body", Vec::new());
            self.doc.append(self.html.unwrap(), b);
            self.stack.push(b);
            self.body = Some(b);
        }
    }

    /// Procura `tag` na pilha de abertos, do topo para baixo, parando nos
    /// limites de escopo e nos nomes em `stop`. Devolve o índice na pilha.
    fn find_in_scope(&self, tag: &str, stop: &[&str]) -> Option<usize> {
        for idx in (0..self.stack.len()).rev() {
            let t = self.tag_at(idx);
            if t == tag {
                return Some(idx);
            }
            if SCOPE_BOUNDARY.contains(&t) || stop.contains(&t) || t == "body" {
                return None;
            }
        }
        None
    }

    fn close_if_open(&mut self, tag: &str, stop: &[&str]) {
        if let Some(idx) = self.find_in_scope(tag, stop) {
            self.stack.truncate(idx);
        }
    }

    fn in_foreign(&self) -> bool {
        self.stack
            .iter()
            .any(|&n| matches!(self.doc.tag(n), Some("svg") | Some("math")))
    }

    fn insert_text(&mut self, text: &str) {
        let parent = self.current();
        if let Some(&last) = self.doc.nodes[parent].children.last()
            && let NodeData::Text(t) = &mut self.doc.nodes[last].data
        {
            t.push_str(text);
            return;
        }
        let n = self.doc.create(NodeData::Text(text.to_string()));
        self.doc.append(parent, n);
    }

    fn process(&mut self, token: Token) {
        let skip_newline = std::mem::take(&mut self.skip_newline);
        match token {
            Token::Doctype(d) => {
                if self.html.is_none() {
                    let n = self.doc.create(NodeData::Doctype(d));
                    self.doc.append(Document::ROOT, n);
                }
            }
            Token::Comment(c) => {
                let n = self.doc.create(NodeData::Comment(c));
                let parent = self.current();
                self.doc.append(parent, n);
            }
            Token::Text(mut t) => {
                if skip_newline {
                    if t.starts_with("\r\n") {
                        t.drain(..2);
                    } else if t.starts_with('\n') {
                        t.drain(..1);
                    }
                    if t.is_empty() {
                        return;
                    }
                }
                if self.body.is_none() {
                    let in_head_text = self
                        .stack
                        .last()
                        .is_some_and(|&n| matches!(self.doc.tag(n), Some("title" | "style" | "script")));
                    if in_head_text {
                        self.insert_text(&t);
                        return;
                    }
                    if t.chars().all(char::is_whitespace) {
                        return;
                    }
                    self.ensure_body();
                }
                self.insert_text(&t);
            }
            Token::StartTag {
                name,
                attrs,
                self_closing,
            } => self.start_tag(name, attrs, self_closing),
            Token::EndTag { name } => self.end_tag(&name),
        }
    }

    fn start_tag(&mut self, name: String, attrs: Vec<(String, String)>, self_closing: bool) {
        match name.as_str() {
            "html" => {
                self.ensure_html();
                // os atributos de <html> vão para o elemento raiz (lang, class, dir, data-*)
                if let Some(h) = self.html {
                    self.merge_attrs(h, attrs);
                }
                return;
            }
            "head" => {
                if self.body.is_none() {
                    self.ensure_head();
                }
                return;
            }
            "body" => {
                if self.body.is_none() {
                    self.ensure_body();
                }
                // <body> criado sozinho ou repetido: junta os atributos que faltam
                if let Some(b) = self.body {
                    self.merge_attrs(b, attrs);
                }
                return;
            }
            _ => {}
        }

        let is_void = VOID.contains(&name.as_str());
        self.skip_newline = matches!(name.as_str(), "pre" | "listing" | "textarea");

        // Conteúdo de <head> antes do <body> vai para o <head>.
        if self.body.is_none() && HEAD_CONTENT.contains(&name.as_str()) {
            self.ensure_head();
            let head = self.head.unwrap();
            let el = self.new_element(&name, attrs);
            self.doc.append(head, el);
            if !is_void && !self_closing {
                self.stack.push(el);
            }
            return;
        }

        self.ensure_body();
        self.close_implied(&name);

        let el = self.new_element(&name, attrs);
        let parent = self.current();
        self.doc.append(parent, el);
        let foreign_self_closing = self_closing && (self.in_foreign() || name == "svg" || name == "math");
        if !is_void && !foreign_self_closing {
            self.stack.push(el);
        }
    }

    /// Fechamentos implícitos: <p> antes de blocos, <li> antes de <li>, etc.
    fn close_implied(&mut self, name: &str) {
        if CLOSES_P.contains(&name) {
            self.close_if_open("p", &[]);
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if matches!(self.doc.tag(self.current()), Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6")) {
                    self.stack.pop();
                }
            }
            "li" => self.close_if_open("li", &["ul", "ol"]),
            "dd" | "dt" => {
                self.close_if_open("dd", &["dl"]);
                self.close_if_open("dt", &["dl"]);
            }
            "option" => {
                if self.doc.tag(self.current()) == Some("option") {
                    self.stack.pop();
                }
            }
            "optgroup" => {
                if self.doc.tag(self.current()) == Some("option") {
                    self.stack.pop();
                }
                if self.doc.tag(self.current()) == Some("optgroup") {
                    self.stack.pop();
                }
            }
            "tr" => self.close_if_open("tr", &["table", "tbody", "thead", "tfoot"]),
            "td" | "th" => {
                self.close_if_open("td", &["tr", "table"]);
                self.close_if_open("th", &["tr", "table"]);
            }
            "thead" | "tbody" | "tfoot" => {
                for t in ["tr", "thead", "tbody", "tfoot"] {
                    self.close_if_open(t, &["table"]);
                }
            }
            "a" => self.close_if_open("a", &[]),
            "button" => self.close_if_open("button", &[]),
            _ => {}
        }
    }

    fn end_tag(&mut self, name: &str) {
        match name {
            "html" | "body" | "br" => {}
            "head" => {
                if let Some(pos) = self.stack.iter().position(|&n| Some(n) == self.head) {
                    self.stack.truncate(pos);
                }
            }
            _ => {
                // fecha o elemento aberto mais próximo com esse nome,
                // sem nunca fechar <html> ou <body>
                for idx in (0..self.stack.len()).rev() {
                    let t = self.tag_at(idx);
                    if t == "body" || t == "html" {
                        break;
                    }
                    if t == name {
                        self.stack.truncate(idx);
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(doc: &Document, parent: NodeId) -> Vec<String> {
        doc.node(parent)
            .children
            .iter()
            .filter_map(|&c| doc.tag(c).map(str::to_string))
            .collect()
    }

    #[test]
    fn cria_html_head_body() {
        let doc = parse("<title>Oi</title><p>texto");
        let html = doc.find_first("html").unwrap();
        assert_eq!(tags(&doc, html), vec!["head", "body"]);
        let title = doc.find_first("title").unwrap();
        assert_eq!(doc.text_content(title), "Oi");
    }

    #[test]
    fn fecha_p_e_li_implicitamente() {
        let doc = parse("<p>um<p>dois<ul><li>a<li>b</ul>");
        let body = doc.find_first("body").unwrap();
        assert_eq!(tags(&doc, body), vec!["p", "p", "ul"]);
        let ul = doc.find_first("ul").unwrap();
        assert_eq!(tags(&doc, ul), vec!["li", "li"]);
    }

    #[test]
    fn script_e_texto_bruto() {
        let doc = parse("<script>if (a < b && c) { x = '</div>'; }</script><div>ok</div>");
        let s = doc.find_first("script").unwrap();
        assert_eq!(doc.text_content(s), "if (a < b && c) { x = '</div>'; }");
        let div = doc.find_first("div").unwrap();
        assert_eq!(doc.text_content(div), "ok");
    }

    #[test]
    fn entidades() {
        assert_eq!(decode_entities("a &amp; b &lt;3 &#233;&#x41; &naoexiste;"), "a & b <3 éA &naoexiste;");
        assert_eq!(decode_entities("S&atilde;o Paulo"), "São Paulo");
    }

    #[test]
    fn atributos() {
        let doc = parse(r#"<a href="/x?a=1&amp;b=2" class=link data-x='y'>z</a>"#);
        let a = doc.find_first("a").unwrap();
        let e = doc.element(a).unwrap();
        assert_eq!(e.attr("href"), Some("/x?a=1&b=2"));
        assert_eq!(e.attr("class"), Some("link"));
        assert_eq!(e.attr("data-x"), Some("y"));
    }
}
