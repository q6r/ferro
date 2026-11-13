//! Parser de CSS: folhas de estilo, regras, declarações, seletores e media
//! queries. Também faz o casamento (matching) de seletores contra o DOM.

use crate::dom::{Document, NodeId};

#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    pub font_faces: Vec<FontFaceRule>,
}

/// Regra `@font-face`.
#[derive(Debug, Clone, PartialEq)]
pub struct FontFaceRule {
    /// Nome da família em minúsculas, sem aspas.
    pub family: String,
    /// URLs em ordem de preferência (woff2, woff, ttf/otf).
    pub sources: Vec<String>,
    pub weight: (u16, u16),
    pub italic: bool,
    /// O `unicode-range` cobre o alfabeto latino (ou não foi dado)?
    pub latin: bool,
}

/// Pseudo-elementos que geram conteúdo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PseudoElement {
    Before,
    After,
}

/// Conteúdo de `url(...)`, sem aspas.
pub fn css_url(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    let start = lower.find("url(")? + 4;
    let rest = &s[start..];
    let end = rest.find(')')?;
    let inner = rest[..end].trim().trim_matches(|c| c == '"' || c == '\'');
    Some(inner.to_string())
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct MediaContext {
    pub width: f32,
    pub height: f32,
}

static PREFERS_DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Os sites devem ver `prefers-color-scheme: dark`? (configuração "Sites no
/// tema escuro", com um tema escuro no navegador)
pub fn set_prefers_dark(on: bool) {
    PREFERS_DARK.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn prefers_dark() -> bool {
    PREFERS_DARK.load(std::sync::atomic::Ordering::Relaxed)
}

// ------------------------------------------------------------------ seletores

#[derive(Debug, Clone)]
pub struct Selector {
    /// Seletores compostos, da esquerda para a direita.
    pub compounds: Vec<Compound>,
    /// `combinators[i]` liga `compounds[i]` a `compounds[i + 1]`.
    pub combinators: Vec<Combinator>,
    /// `::before` / `::after` no fim do seletor.
    pub pseudo_element: Option<PseudoElement>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Combinator {
    Descendant,
    Child,
    NextSibling,
    SubsequentSibling,
}

#[derive(Debug, Clone, Default)]
pub struct Compound {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSelector>,
    pub pseudos: Vec<Pseudo>,
}

#[derive(Debug, Clone)]
pub struct AttrSelector {
    pub name: String,
    pub op: AttrOp,
    pub value: String,
    pub case_insensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttrOp {
    Exists,
    Equals,
    Includes,
    DashMatch,
    Prefix,
    Suffix,
    Substring,
}

#[derive(Debug, Clone)]
pub enum Pseudo {
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    NthChild(i32, i32),
    NthLastChild(i32, i32),
    NthOfType(i32, i32),
    Root,
    Link,
    Empty,
    Checked,
    Disabled,
    Enabled,
    Not(Vec<Selector>),
    Is(Vec<Selector>),
    Where(Vec<Selector>),
    /// Estados dinâmicos (dependem do mouse e do foco).
    Hover,
    Active,
    Focus,
    FocusVisible,
    FocusWithin,
    /// :placeholder-shown (campo vazio com placeholder).
    PlaceholderShown,
    /// :fullscreen (o elemento em tela cheia).
    Fullscreen,
    /// :popover-open (popover aberto com showPopover).
    PopoverOpen,
    /// Pseudo-classe que não sabemos avaliar: nunca casa.
    Never,
}

/// Estado do mouse e do foco, para `:hover`, `:active`, `:focus`...
#[derive(Default, Clone, Debug, PartialEq)]
pub struct DynState {
    /// Elemento sob o mouse e os ancestrais dele (árvore achatada).
    pub hover: Vec<NodeId>,
    /// Elemento com o botão do mouse apertado e os ancestrais.
    pub active: Vec<NodeId>,
    pub focus: Option<NodeId>,
    /// O foco veio do teclado (ou é um campo de texto): `:focus-visible`.
    pub focus_visible: bool,
    /// Elemento com foco e os ancestrais.
    pub focus_within: Vec<NodeId>,
    /// Elemento em tela cheia.
    pub fullscreen: Option<NodeId>,
    /// Popovers abertos.
    pub popover_open: Vec<NodeId>,
}

thread_local! {
    static DYN: std::cell::RefCell<DynState> = std::cell::RefCell::new(DynState::default());
}

/// Diz aos seletores onde estão o mouse e o foco agora.
pub fn set_dynamic_state(s: DynState) {
    DYN.with(|d| *d.borrow_mut() = s);
}

/// Cópia do estado dinâmico atual (para saber o que mudou entre dois cálculos).
pub fn dynamic_state() -> DynState {
    DYN.with(|d| d.borrow().clone())
}

fn dyn_state<R>(f: impl FnOnce(&DynState) -> R) -> R {
    DYN.with(|d| f(&d.borrow()))
}

/// Especificidade (ids, classes/atributos/pseudo-classes, tags).
pub type Specificity = (u32, u32, u32);

impl Selector {
    pub fn specificity(&self) -> Specificity {
        let mut s = (0, 0, self.pseudo_element.is_some() as u32);
        for c in &self.compounds {
            let cs = c.specificity();
            s = (s.0 + cs.0, s.1 + cs.1, s.2 + cs.2);
        }
        s
    }
}

impl Compound {
    fn specificity(&self) -> Specificity {
        let mut a = self.id.is_some() as u32;
        let mut b = (self.classes.len() + self.attrs.len()) as u32;
        let mut c = self.tag.is_some() as u32;
        for p in &self.pseudos {
            match p {
                Pseudo::Where(_) => {}
                Pseudo::Not(list) | Pseudo::Is(list) => {
                    let m = list.iter().map(Selector::specificity).max().unwrap_or((0, 0, 0));
                    a += m.0;
                    b += m.1;
                    c += m.2;
                }
                Pseudo::Never => b += 1,
                _ => b += 1,
            }
        }
        (a, b, c)
    }
}

pub fn parse_selector_list(src: &str) -> Option<Vec<Selector>> {
    let mut out = Vec::new();
    for part in split_top_level(src, ',') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        out.push(parse_selector(part)?);
    }
    if out.is_empty() { None } else { Some(out) }
}

fn parse_selector(src: &str) -> Option<Selector> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut compounds = Vec::new();
    let mut combinators = Vec::new();
    let mut current = Compound::default();
    let mut has_current = false;
    let mut pending_comb: Option<Combinator> = None;
    let mut pseudo_element: Option<PseudoElement> = None;

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || matches!(c, '>' | '+' | '~') {
            // combinador
            let mut comb = Combinator::Descendant;
            while i < chars.len() && (chars[i].is_whitespace() || matches!(chars[i], '>' | '+' | '~')) {
                match chars[i] {
                    '>' => comb = Combinator::Child,
                    '+' => comb = Combinator::NextSibling,
                    '~' => comb = Combinator::SubsequentSibling,
                    _ => {}
                }
                i += 1;
            }
            if has_current {
                compounds.push(std::mem::take(&mut current));
                has_current = false;
                pending_comb = Some(comb);
            } else if compounds.is_empty() {
                // seletor relativo (ex.: "> a") não é suportado aqui
                return None;
            }
            continue;
        }
        if let Some(comb) = pending_comb.take() {
            combinators.push(comb);
        }
        has_current = true;
        match c {
            '*' => i += 1,
            '#' => {
                i += 1;
                current.id = Some(read_ident(&chars, &mut i)?);
            }
            '.' => {
                i += 1;
                current.classes.push(read_ident(&chars, &mut i)?);
            }
            '[' => {
                i += 1;
                current.attrs.push(parse_attr_selector(&chars, &mut i)?);
            }
            ':' => {
                i += 1;
                let element = chars.get(i) == Some(&':');
                if element {
                    i += 1;
                }
                let name = read_ident(&chars, &mut i)?.to_ascii_lowercase();
                let mut arg = None;
                if chars.get(i) == Some(&'(') {
                    i += 1;
                    let start = i;
                    let mut depth = 1;
                    while i < chars.len() && depth > 0 {
                        match chars[i] {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    if depth != 0 {
                        return None;
                    }
                    arg = Some(chars[start..i - 1].iter().collect::<String>());
                }
                match name.as_str() {
                    "before" if arg.is_none() => pseudo_element = Some(PseudoElement::Before),
                    "after" if arg.is_none() => pseudo_element = Some(PseudoElement::After),
                    _ if element || matches!(name.as_str(), "first-line" | "first-letter") => {
                        current.pseudos.push(Pseudo::Never)
                    }
                    _ => current.pseudos.push(parse_pseudo(&name, arg.as_deref())),
                }
            }
            _ if is_ident_start(c) => {
                current.tag = Some(read_ident(&chars, &mut i)?.to_ascii_lowercase());
            }
            _ => return None,
        }
    }
    if !has_current {
        return None;
    }
    compounds.push(current);
    if combinators.len() + 1 != compounds.len() {
        return None;
    }
    Some(Selector {
        compounds,
        combinators,
        pseudo_element,
    })
}

fn parse_pseudo(name: &str, arg: Option<&str>) -> Pseudo {
    match (name, arg) {
        ("first-child", None) => Pseudo::FirstChild,
        ("last-child", None) => Pseudo::LastChild,
        ("only-child", None) => Pseudo::OnlyChild,
        ("first-of-type", None) => Pseudo::FirstOfType,
        ("last-of-type", None) => Pseudo::LastOfType,
        ("root", None) => Pseudo::Root,
        ("link" | "any-link", None) => Pseudo::Link,
        ("empty", None) => Pseudo::Empty,
        ("checked", None) => Pseudo::Checked,
        ("disabled", None) => Pseudo::Disabled,
        ("enabled", None) => Pseudo::Enabled,
        ("nth-child", Some(a)) => parse_nth(a).map_or(Pseudo::Never, |(a, b)| Pseudo::NthChild(a, b)),
        ("nth-last-child", Some(a)) => parse_nth(a).map_or(Pseudo::Never, |(a, b)| Pseudo::NthLastChild(a, b)),
        ("nth-of-type", Some(a)) => parse_nth(a).map_or(Pseudo::Never, |(a, b)| Pseudo::NthOfType(a, b)),
        ("not", Some(a)) => parse_selector_list(a).map_or(Pseudo::Never, Pseudo::Not),
        ("is" | "matches" | "-webkit-any", Some(a)) => {
            parse_selector_list(a).map_or(Pseudo::Never, Pseudo::Is)
        }
        ("where", Some(a)) => parse_selector_list(a).map_or(Pseudo::Never, Pseudo::Where),
        ("hover", None) => Pseudo::Hover,
        ("active", None) => Pseudo::Active,
        ("focus", None) => Pseudo::Focus,
        ("focus-visible", None) => Pseudo::FocusVisible,
        ("focus-within", None) => Pseudo::FocusWithin,
        ("placeholder-shown", None) => Pseudo::PlaceholderShown,
        ("fullscreen" | "-webkit-full-screen" | "modal", None) => Pseudo::Fullscreen,
        ("popover-open", None) => Pseudo::PopoverOpen,
        // :visited, :has(), :first-line...
        _ => Pseudo::Never,
    }
}

/// Microsintaxe An+B (odd, even, 3, 2n+1, -n+3).
fn parse_nth(s: &str) -> Option<(i32, i32)> {
    let s: String = s.split_whitespace().collect::<String>().to_ascii_lowercase();
    match s.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Some(npos) = s.find('n') {
        let a_part = &s[..npos];
        let a = match a_part {
            "" | "+" => 1,
            "-" => -1,
            x => x.parse().ok()?,
        };
        let b_part = &s[npos + 1..];
        let b = if b_part.is_empty() {
            0
        } else {
            b_part.trim_start_matches('+').parse().ok()?
        };
        Some((a, b))
    } else {
        Some((0, s.parse().ok()?))
    }
}

fn parse_attr_selector(chars: &[char], i: &mut usize) -> Option<AttrSelector> {
    skip_ws(chars, i);
    let name = read_ident(chars, i)?.to_ascii_lowercase();
    skip_ws(chars, i);
    let op = match chars.get(*i)? {
        ']' => {
            *i += 1;
            return Some(AttrSelector {
                name,
                op: AttrOp::Exists,
                value: String::new(),
                case_insensitive: false,
            });
        }
        '=' => AttrOp::Equals,
        '~' => AttrOp::Includes,
        '|' => AttrOp::DashMatch,
        '^' => AttrOp::Prefix,
        '$' => AttrOp::Suffix,
        '*' => AttrOp::Substring,
        _ => return None,
    };
    *i += if op == AttrOp::Equals { 1 } else { 2 };
    skip_ws(chars, i);
    let value = match chars.get(*i)? {
        q @ ('"' | '\'') => {
            let q = *q;
            *i += 1;
            let mut v = String::new();
            while *i < chars.len() && chars[*i] != q {
                if chars[*i] == '\\' && *i + 1 < chars.len() {
                    *i += 1;
                }
                v.push(chars[*i]);
                *i += 1;
            }
            *i += 1;
            v
        }
        _ => read_ident(chars, i)?,
    };
    skip_ws(chars, i);
    let mut case_insensitive = false;
    if matches!(chars.get(*i), Some('i' | 'I')) {
        case_insensitive = true;
        *i += 1;
        skip_ws(chars, i);
    } else if matches!(chars.get(*i), Some('s' | 'S')) {
        *i += 1;
        skip_ws(chars, i);
    }
    if chars.get(*i) != Some(&']') {
        return None;
    }
    *i += 1;
    Some(AttrSelector {
        name,
        op,
        value,
        case_insensitive,
    })
}

fn skip_ws(chars: &[char], i: &mut usize) {
    while *i < chars.len() && chars[*i].is_whitespace() {
        *i += 1;
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '-' || c == '\\' || !c.is_ascii()
}

/// Lê um identificador CSS, tratando escapes (`\:`, `\31 `).
fn read_ident(chars: &[char], i: &mut usize) -> Option<String> {
    let mut s = String::new();
    while *i < chars.len() {
        let c = chars[*i];
        if c == '\\' {
            *i += 1;
            let hex: String = chars[*i..].iter().take(6).take_while(|c| c.is_ascii_hexdigit()).collect();
            if !hex.is_empty() {
                *i += hex.len();
                if chars.get(*i).is_some_and(|c| c.is_whitespace()) {
                    *i += 1;
                }
                s.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?).unwrap_or('\u{FFFD}'));
            } else if let Some(&next) = chars.get(*i) {
                s.push(next);
                *i += 1;
            }
        } else if c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii() {
            s.push(c);
            *i += 1;
        } else {
            break;
        }
    }
    if s.is_empty() { None } else { Some(s) }
}

// ------------------------------------------------------------------ matching

pub fn matches(doc: &Document, node: NodeId, sel: &Selector) -> bool {
    match_from(doc, node, sel, sel.compounds.len() - 1)
}

fn match_from(doc: &Document, node: NodeId, sel: &Selector, idx: usize) -> bool {
    if !match_compound(doc, node, &sel.compounds[idx]) {
        return false;
    }
    if idx == 0 {
        return true;
    }
    match sel.combinators[idx - 1] {
        Combinator::Child => doc
            .parent_element(node)
            .is_some_and(|p| match_from(doc, p, sel, idx - 1)),
        Combinator::Descendant => {
            let mut cur = doc.parent_element(node);
            while let Some(p) = cur {
                if match_from(doc, p, sel, idx - 1) {
                    return true;
                }
                cur = doc.parent_element(p);
            }
            false
        }
        Combinator::NextSibling => {
            prev_elements(doc, node).next().is_some_and(|p| match_from(doc, p, sel, idx - 1))
        }
        Combinator::SubsequentSibling => prev_elements(doc, node).any(|p| match_from(doc, p, sel, idx - 1)),
    }
}

/// Irmãos elementos antes do nó, do mais próximo para o mais distante.
fn prev_elements(doc: &Document, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    let sibs: &[NodeId] = match doc.node(node).parent {
        Some(p) => &doc.node(p).children,
        None => &[],
    };
    let at = sibs.iter().position(|&s| s == node).unwrap_or(0);
    sibs[..at].iter().rev().copied().filter(|&c| doc.element(c).is_some())
}

/// Posição do nó entre os irmãos elementos (a partir de 1) e quantos são;
/// com `tag`, conta só os dessa tag.
fn element_position(doc: &Document, node: NodeId, tag: Option<&str>) -> (i32, i32) {
    let Some(p) = doc.node(node).parent else { return (1, 1) };
    let (mut pos, mut count) = (0, 0);
    for &c in &doc.node(p).children {
        let Some(e) = doc.element(c) else { continue };
        if tag.is_some_and(|t| e.tag != t) {
            continue;
        }
        count += 1;
        if c == node {
            pos = count;
        }
    }
    (pos.max(1), count.max(1))
}

fn nth_matches(a: i32, b: i32, pos: i32) -> bool {
    // existe n >= 0 com a*n + b == pos?
    if a == 0 {
        return pos == b;
    }
    let diff = pos - b;
    diff % a == 0 && diff / a >= 0
}

fn match_compound(doc: &Document, node: NodeId, c: &Compound) -> bool {
    let Some(el) = doc.element(node) else {
        return false;
    };
    if let Some(tag) = &c.tag
        && *tag != el.tag
    {
        return false;
    }
    if let Some(id) = &c.id
        && el.id() != Some(id.as_str())
    {
        return false;
    }
    if !c.classes.iter().all(|cl| el.has_class(cl)) {
        return false;
    }
    for a in &c.attrs {
        let Some(v) = el.attr(&a.name) else {
            return false;
        };
        let (v, want) = if a.case_insensitive {
            (v.to_lowercase(), a.value.to_lowercase())
        } else {
            (v.to_string(), a.value.clone())
        };
        let ok = match a.op {
            AttrOp::Exists => true,
            AttrOp::Equals => v == want,
            AttrOp::Includes => v.split_ascii_whitespace().any(|x| x == want),
            AttrOp::DashMatch => v == want || v.starts_with(&format!("{want}-")),
            AttrOp::Prefix => !want.is_empty() && v.starts_with(&want),
            AttrOp::Suffix => !want.is_empty() && v.ends_with(&want),
            AttrOp::Substring => !want.is_empty() && v.contains(&want),
        };
        if !ok {
            return false;
        }
    }
    for p in &c.pseudos {
        let ok = match p {
            Pseudo::Never => false,
            Pseudo::Hover => dyn_state(|d| d.hover.contains(&node)),
            Pseudo::Active => dyn_state(|d| d.active.contains(&node)),
            Pseudo::Focus => dyn_state(|d| d.focus == Some(node)),
            Pseudo::FocusVisible => dyn_state(|d| d.focus == Some(node) && d.focus_visible),
            Pseudo::FocusWithin => dyn_state(|d| d.focus_within.contains(&node)),
            Pseudo::Fullscreen => dyn_state(|d| d.fullscreen == Some(node)),
            Pseudo::PopoverOpen => dyn_state(|d| d.popover_open.contains(&node)),
            Pseudo::PlaceholderShown => {
                el.attr("placeholder").is_some()
                    && match el.tag.as_str() {
                        "input" => el.attr("value").is_none_or(str::is_empty),
                        "textarea" => doc.text_content(node).is_empty(),
                        _ => false,
                    }
            }
            Pseudo::Root => doc.parent_element(node).is_none(),
            Pseudo::Link => matches!(el.tag.as_str(), "a" | "area") && el.attr("href").is_some(),
            Pseudo::Empty => doc.node(node).children.iter().all(|&ch| match &doc.node(ch).data {
                crate::dom::NodeData::Text(t) => t.is_empty(),
                crate::dom::NodeData::Comment(_) => true,
                _ => false,
            }),
            Pseudo::Checked => el.attr("checked").is_some() || el.attr("selected").is_some(),
            Pseudo::Disabled => el.attr("disabled").is_some(),
            Pseudo::Enabled => {
                matches!(el.tag.as_str(), "input" | "button" | "select" | "textarea")
                    && el.attr("disabled").is_none()
            }
            Pseudo::FirstChild
            | Pseudo::LastChild
            | Pseudo::OnlyChild
            | Pseudo::NthChild(..)
            | Pseudo::NthLastChild(..) => {
                let (pos, count) = element_position(doc, node, None);
                let last = count - pos + 1;
                match p {
                    Pseudo::FirstChild => pos == 1,
                    Pseudo::LastChild => last == 1,
                    Pseudo::OnlyChild => count == 1,
                    Pseudo::NthChild(a, b) => nth_matches(*a, *b, pos),
                    Pseudo::NthLastChild(a, b) => nth_matches(*a, *b, last),
                    _ => unreachable!(),
                }
            }
            Pseudo::FirstOfType | Pseudo::LastOfType | Pseudo::NthOfType(..) => {
                let (pos, count) = element_position(doc, node, Some(el.tag.as_str()));
                match p {
                    Pseudo::FirstOfType => pos == 1,
                    Pseudo::LastOfType => pos == count,
                    Pseudo::NthOfType(a, b) => nth_matches(*a, *b, pos),
                    _ => unreachable!(),
                }
            }
            Pseudo::Not(list) => !list.iter().any(|s| matches(doc, node, s)),
            Pseudo::Is(list) | Pseudo::Where(list) => list.iter().any(|s| matches(doc, node, s)),
        };
        if !ok {
            return false;
        }
    }
    true
}

// ------------------------------------------------------------ folha de estilo

pub fn parse_stylesheet(src: &str, ctx: &MediaContext) -> Stylesheet {
    let clean = strip_comments(src);
    let chars: Vec<char> = clean.chars().collect();
    let mut p = Parser {
        s: &chars,
        pos: 0,
        ctx,
        faces: Vec::new(),
    };
    let mut rules = Vec::new();
    p.parse_rule_list(&mut rules, false);
    Stylesheet {
        rules,
        font_faces: p.faces,
    }
}

/// Declarações soltas, como no atributo `style="..."`.
pub fn parse_declarations(src: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    for part in split_top_level(&strip_comments(src), ';') {
        let Some(colon) = part.find(':') else { continue };
        let name = part[..colon].trim();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            continue;
        }
        let mut value = part[colon + 1..].trim().to_string();
        let mut important = false;
        if let Some(bang) = value.rfind('!')
            && value[bang + 1..].trim().eq_ignore_ascii_case("important")
        {
            important = true;
            value = value[..bang].trim().to_string();
        }
        if value.is_empty() {
            continue;
        }
        let name = if name.starts_with("--") {
            name.to_string()
        } else {
            name.to_ascii_lowercase()
        };
        out.push(Declaration {
            name,
            value,
            important,
        });
    }
    out
}

struct Parser<'a> {
    s: &'a [char],
    pos: usize,
    ctx: &'a MediaContext,
    faces: Vec<FontFaceRule>,
}

impl Parser<'_> {
    fn eof(&self) -> bool {
        self.pos >= self.s.len()
    }

    fn parse_rule_list(&mut self, rules: &mut Vec<Rule>, nested: bool) {
        loop {
            while !self.eof() && self.s[self.pos].is_whitespace() {
                self.pos += 1;
            }
            if self.eof() {
                return;
            }
            match self.s[self.pos] {
                '}' => {
                    self.pos += 1;
                    if nested {
                        return;
                    }
                }
                '@' => self.parse_at_rule(rules),
                _ => {
                    // marcadores antigos <!-- e --> dentro de <style>
                    if self.starts_with("<!--") {
                        self.pos += 4;
                        continue;
                    }
                    if self.starts_with("-->") {
                        self.pos += 3;
                        continue;
                    }
                    let (prelude, stop) = self.read_prelude();
                    if stop != Some('{') {
                        continue; // lixo antes de ';' ou fim
                    }
                    self.pos += 1;
                    let block = self.read_block();
                    if let Some(selectors) = parse_selector_list(&prelude) {
                        let declarations = parse_declarations(&block);
                        if !declarations.is_empty() {
                            rules.push(Rule {
                                selectors,
                                declarations,
                            });
                        }
                    }
                }
            }
        }
    }

    fn starts_with(&self, pat: &str) -> bool {
        pat.chars().enumerate().all(|(k, c)| self.s.get(self.pos + k) == Some(&c))
    }

    /// Lê até '{' ou ';' fora de parênteses/aspas. Não consome o delimitador.
    fn read_prelude(&mut self) -> (String, Option<char>) {
        let start = self.pos;
        let mut depth = 0;
        let mut quote: Option<char> = None;
        while !self.eof() {
            let c = self.s[self.pos];
            if let Some(q) = quote {
                if c == '\\' {
                    self.pos += 1;
                } else if c == q {
                    quote = None;
                }
            } else {
                match c {
                    '"' | '\'' => quote = Some(c),
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth -= 1,
                    '{' | ';' if depth <= 0 => {
                        let prelude = self.s[start..self.pos].iter().collect();
                        if c == ';' {
                            self.pos += 1;
                        }
                        return (prelude, Some(c));
                    }
                    '}' if depth <= 0 => {
                        return (self.s[start..self.pos].iter().collect(), None);
                    }
                    _ => {}
                }
            }
            self.pos += 1;
        }
        (self.s[start..].iter().collect(), None)
    }

    /// Lê o conteúdo de um bloco já aberto até o '}' correspondente (consumido).
    fn read_block(&mut self) -> String {
        let start = self.pos;
        let mut depth = 1;
        let mut quote: Option<char> = None;
        while !self.eof() {
            let c = self.s[self.pos];
            if let Some(q) = quote {
                if c == '\\' {
                    self.pos += 1;
                } else if c == q {
                    quote = None;
                }
            } else {
                match c {
                    '"' | '\'' => quote = Some(c),
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let block = self.s[start..self.pos].iter().collect();
                            self.pos += 1;
                            return block;
                        }
                    }
                    _ => {}
                }
            }
            self.pos += 1;
        }
        self.s[start..].iter().collect()
    }

    fn parse_at_rule(&mut self, rules: &mut Vec<Rule>) {
        self.pos += 1;
        let mut name = String::new();
        while !self.eof() && (self.s[self.pos].is_alphanumeric() || self.s[self.pos] == '-') {
            name.push(self.s[self.pos].to_ascii_lowercase());
            self.pos += 1;
        }
        let (prelude, stop) = self.read_prelude();
        if stop != Some('{') {
            return; // @import, @charset, @layer a, b; ...
        }
        self.pos += 1;
        let include = match name.as_str() {
            "media" => media_matches(&prelude, self.ctx),
            "supports" => supports_matches(&prelude),
            "layer" => true,
            _ => false, // @font-face, @keyframes, @page, @container...
        };
        if include {
            self.parse_rule_list(rules, true);
        } else if name == "font-face" {
            let block = self.read_block();
            if let Some(f) = parse_font_face(&block) {
                self.faces.push(f);
            }
        } else {
            self.read_block();
        }
    }
}

// --------------------------------------------------------------- @supports

/// Propriedades que o motor entende (para avaliar `@supports`).
const SUPPORTED_PROPERTIES: &[&str] = &[
    "color", "background", "background-color", "margin", "margin-top", "margin-right", "margin-bottom",
    "margin-left", "margin-block", "margin-inline", "padding", "padding-top", "padding-right",
    "padding-bottom", "padding-left", "padding-block", "padding-inline", "border", "border-top",
    "border-right", "border-bottom", "border-left", "border-width", "border-style", "border-color",
    "width", "height", "min-width", "min-height", "max-width", "font", "font-size", "font-weight",
    "font-style", "font-family", "line-height", "text-align", "text-decoration", "text-decoration-line",
    "text-transform", "white-space", "visibility", "opacity", "z-index", "top", "left", "right", "bottom",
    "inset", "position", "display", "flex", "flex-direction", "flex-wrap", "flex-flow", "flex-grow",
    "flex-shrink", "flex-basis", "justify-content", "align-items", "align-self", "gap", "row-gap",
    "column-gap", "order", "overflow", "overflow-x", "overflow-y", "box-sizing", "list-style",
    "list-style-type",
];

fn supports_declaration(name: &str, value: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    let value = value.trim().to_ascii_lowercase();
    if name.starts_with("--") {
        return true;
    }
    // funções de valor que ainda não existem
    const UNSUPPORTED_FUNCS: &[&str] = &[
        "min(", "max(", "clamp(", "color-mix(", "env(", "lab(", "lch(", "oklab(", "oklch(", "attr(",
        "anchor(", "round(",
    ];
    if UNSUPPORTED_FUNCS.iter().any(|f| value.contains(f)) {
        return false;
    }
    match name.as_str() {
        "display" => matches!(
            value.split_whitespace().next().unwrap_or(""),
            "none" | "block" | "inline" | "inline-block" | "flex" | "inline-flex" | "list-item" | "contents"
                | "flow-root" | "table" | "table-row" | "table-cell"
        ),
        "position" => matches!(value.as_str(), "static" | "relative" | "absolute" | "fixed"),
        _ => SUPPORTED_PROPERTIES.contains(&name.as_str()),
    }
}

/// Divide por uma palavra-chave (" and ", " or ") fora de parênteses.
fn split_keyword<'a>(s: &'a str, kw: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && s[i..].starts_with(kw) {
            parts.push(&s[start..i]);
            i += kw.len();
            start = i;
            continue;
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

/// Avalia a condição de um `@supports`.
pub fn supports_matches(cond: &str) -> bool {
    let c = cond.trim();
    let lower = c.to_ascii_lowercase();
    let ors = split_keyword(&lower, " or ");
    if ors.len() > 1 {
        return ors.iter().any(|p| supports_matches(p));
    }
    let ands = split_keyword(&lower, " and ");
    if ands.len() > 1 {
        return ands.iter().all(|p| supports_matches(p));
    }
    if let Some(rest) = lower.strip_prefix("not") {
        let rest = rest.trim_start();
        if rest.starts_with('(') {
            return !supports_matches(rest);
        }
    }
    if let Some(inner) = lower.strip_prefix("selector(").and_then(|r| r.strip_suffix(')')) {
        return parse_selector_list(inner).is_some();
    }
    if let Some(inner) = lower.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        let inner = inner.trim();
        if inner.starts_with('(') || inner.starts_with("not") || inner.starts_with("selector(") {
            return supports_matches(inner);
        }
        return match inner.split_once(':') {
            Some((n, v)) => supports_declaration(n, v),
            None => false,
        };
    }
    false
}

fn parse_font_face(block: &str) -> Option<FontFaceRule> {
    let decls = parse_declarations(block);
    let get = |n: &str| decls.iter().find(|d| d.name == n).map(|d| d.value.as_str());
    let family = get("font-family")?
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_ascii_lowercase();
    let mut sources: Vec<(u8, String)> = Vec::new();
    for part in split_top_level(get("src")?, ',') {
        let Some(url) = css_url(&part) else { continue };
        let lower = part.to_ascii_lowercase();
        let rank = if lower.contains("woff2") {
            0
        } else if lower.contains("woff") {
            1
        } else if lower.contains("truetype") || lower.contains("opentype") || url.ends_with(".ttf") || url.ends_with(".otf") {
            2
        } else if lower.contains("embedded-opentype") || lower.contains("format(\"svg") || url.ends_with(".eot") {
            continue;
        } else {
            3
        };
        sources.push((rank, url));
    }
    sources.sort_by_key(|s| s.0);
    let weight = match get("font-weight").map(str::trim) {
        None | Some("normal") => (400, 400),
        Some("bold") => (700, 700),
        Some(w) => {
            let nums: Vec<u16> = w.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            match nums.as_slice() {
                [a] => (*a, *a),
                [a, b, ..] => (*a, *b),
                _ => (400, 400),
            }
        }
    };
    let italic = get("font-style").is_some_and(|v| v.contains("italic") || v.contains("oblique"));
    if sources.is_empty() {
        return None;
    }
    // unicode-range: U+0000-00FF, U+0131, U+2000-206F...
    let latin = match get("unicode-range") {
        None => true,
        Some(r) => r.split(',').any(|part| {
            let p = part.trim().trim_start_matches(['U', 'u']).trim_start_matches('+');
            let (a, b) = match p.split_once('-') {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None if p.contains('?') => (p.replace('?', "0"), p.replace('?', "F")),
                None => (p.to_string(), p.to_string()),
            };
            match (u32::from_str_radix(&a, 16), u32::from_str_radix(&b, 16)) {
                (Ok(a), Ok(b)) => a <= 0x7A && b >= 0x41,
                _ => false,
            }
        }),
    };
    Some(FontFaceRule {
        family,
        sources: sources.into_iter().map(|s| s.1).collect(),
        weight,
        italic,
        latin,
    })
}

pub fn media_matches(query: &str, ctx: &MediaContext) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    split_top_level(&q, ',').iter().any(|part| single_media_matches(part.trim(), ctx))
}

fn single_media_matches(q: &str, ctx: &MediaContext) -> bool {
    let (negate, q) = match q.strip_prefix("not ") {
        Some(rest) => (true, rest.trim()),
        None => (false, q.strip_prefix("only ").unwrap_or(q).trim()),
    };
    let mut result = true;
    for cond in q.split(" and ") {
        let cond = cond.trim();
        let ok = match cond {
            "" | "all" | "screen" => true,
            "print" | "speech" | "tv" | "projection" | "handheld" => false,
            _ if cond.starts_with('(') => feature_matches(cond.trim_start_matches('(').trim_end_matches(')'), ctx),
            _ => false,
        };
        result &= ok;
    }
    result != negate
}

fn media_length(v: &str) -> Option<f32> {
    let v = v.trim();
    if let Some(n) = v.strip_suffix("px") {
        n.trim().parse().ok()
    } else if let Some(n) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        n.trim().parse::<f32>().ok().map(|x| x * 16.0)
    } else {
        v.parse().ok()
    }
}

fn feature_matches(f: &str, ctx: &MediaContext) -> bool {
    // sintaxe de intervalo: (width >= 600px), (400px <= width <= 700px)
    for op in [">=", "<=", ">", "<"] {
        if f.contains(op) {
            let parts: Vec<&str> = f.split(op).map(str::trim).collect();
            if parts.len() == 2 {
                let (name, val, flip) = if parts[0] == "width" || parts[0] == "height" {
                    (parts[0], parts[1], false)
                } else {
                    (parts[1], parts[0], true)
                };
                let actual = if name == "width" { ctx.width } else { ctx.height };
                let Some(v) = media_length(val) else { return false };
                let op = if flip {
                    match op {
                        ">=" => "<=",
                        "<=" => ">=",
                        ">" => "<",
                        _ => ">",
                    }
                } else {
                    op
                };
                return match op {
                    ">=" => actual >= v,
                    "<=" => actual <= v,
                    ">" => actual > v,
                    _ => actual < v,
                };
            }
            return false;
        }
    }
    let (name, value) = match f.split_once(':') {
        Some((n, v)) => (n.trim(), v.trim()),
        None => (f.trim(), ""),
    };
    match name {
        "min-width" => media_length(value).is_some_and(|v| ctx.width >= v),
        "max-width" => media_length(value).is_some_and(|v| ctx.width <= v),
        "min-height" => media_length(value).is_some_and(|v| ctx.height >= v),
        "max-height" => media_length(value).is_some_and(|v| ctx.height <= v),
        "orientation" => (value == "landscape") == (ctx.width >= ctx.height),
        "prefers-color-scheme" => value == if prefers_dark() { "dark" } else { "light" },
        "prefers-reduced-motion" | "prefers-contrast" | "forced-colors" => value == "no-preference" || value == "none",
        "hover" | "any-hover" => value == "hover" || value.is_empty(),
        "pointer" | "any-pointer" => value == "fine" || value.is_empty(),
        "color" => true,
        _ => false,
    }
}

fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Divide por `sep` ignorando o que estiver entre parênteses, colchetes ou aspas.
pub fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' {
            cur.push(c);
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            cur.push(c);
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if c == sep && depth <= 0 => {
                parts.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html;

    const CTX: MediaContext = MediaContext {
        width: 1280.0,
        height: 800.0,
    };

    #[test]
    fn regras_e_declaracoes() {
        let s = parse_stylesheet(
            "/* c */ h1, .a > p { color: red; margin: 0 auto !important } @media print { p { color: blue } }",
            &CTX,
        );
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors.len(), 2);
        assert_eq!(s.rules[0].declarations[1].name, "margin");
        assert!(s.rules[0].declarations[1].important);
    }

    #[test]
    fn media_queries() {
        assert!(media_matches("screen and (min-width: 768px)", &CTX));
        assert!(!media_matches("(max-width: 600px)", &CTX));
        assert!(media_matches("(width >= 1000px)", &CTX));
        assert!(!media_matches("print", &CTX));
    }

    #[test]
    fn especificidade() {
        let s = &parse_selector_list("#a .b p:first-child").unwrap()[0];
        assert_eq!(s.specificity(), (1, 2, 1));
    }

    #[test]
    fn casamento_de_seletores() {
        let doc = html::parse(
            r#"<div id="x" class="c1 c2"><ul><li>a</li><li class="z">b</li></ul></div><p lang="pt-BR">t</p>"#,
        );
        let lis = doc.find_all("li");
        let p = doc.find_first("p").unwrap();
        let sel = |s: &str| parse_selector_list(s).unwrap().remove(0);
        assert!(matches(&doc, lis[1], &sel("#x.c1 li.z")));
        assert!(matches(&doc, lis[0], &sel("ul > li:first-child")));
        assert!(matches(&doc, lis[1], &sel("li + li")));
        assert!(!matches(&doc, lis[0], &sel("li:nth-child(2n)")));
        assert!(matches(&doc, lis[1], &sel("li:not(:first-child)")));
        assert!(matches(&doc, p, &sel("div ~ p[lang|=pt]")));
        assert!(!matches(&doc, p, &sel("a:hover")));
        assert!(matches(&doc, lis[0], &sel(".c1 :is(li, span)")));
    }

    #[test]
    fn pseudo_elementos_e_font_face() {
        let s = &parse_selector_list(".a > p::before").unwrap()[0];
        assert_eq!(s.pseudo_element, Some(PseudoElement::Before));
        assert_eq!(s.specificity(), (0, 1, 2));
        let sheet = parse_stylesheet(
            "@font-face { font-family: 'Fira Sans'; src: url(a.woff) format('woff'), url(a.woff2) format('woff2'); font-weight: 700 }",
            &CTX,
        );
        assert_eq!(sheet.font_faces[0].family, "fira sans");
        assert_eq!(sheet.font_faces[0].sources[0], "a.woff2");
        assert_eq!(sheet.font_faces[0].weight, (700, 700));
    }

    #[test]
    fn supports() {
        assert!(supports_matches("(display: flex)"));
        assert!(!supports_matches("(display: grid)"));
        assert!(supports_matches("not (display: grid)"));
        assert!(supports_matches("(display: grid) or (display: flex)"));
        assert!(!supports_matches("(display: flex) and (width: clamp(1px, 2vw, 3px))"));
        assert!(supports_matches("selector(:is(a, b))"));
    }

    #[test]
    fn escapes_em_classes() {
        let s = &parse_selector_list(r".md\:flex").unwrap()[0];
        assert_eq!(s.compounds[0].classes[0], "md:flex");
    }
}
