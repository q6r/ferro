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
}

thread_local! {
    static DYN: std::cell::RefCell<DynState> = std::cell::RefCell::new(DynState::default());
}

/// Diz aos seletores onde estão o mouse e o foco agora.
pub fn set_dynamic_state(s: DynState) {
    DYN.with(|d| *d.borrow_mut() = s);
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

