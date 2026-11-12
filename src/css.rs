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

