//! Motor de expressões regulares do JavaScript.
//!
//! Padrão (sintaxe ECMAScript, com as regras legadas do Anexo B) → árvore →
//! bytecode → máquina de backtracking com pilha explícita (sem recursão, para
//! não estourar a pilha com `.*` em textos grandes). Repetições de um único
//! caractere (`a*`, `[a-z]+`, `.*?`) têm instrução própria, que guarda um só
//! ponto de retorno em vez de um por caractere.
//!
//! O texto é UTF-16 (como as strings do JS). Sem a flag `u`, casa unidades;
//! com `u`/`v`, casa pontos de código (pares substitutos juntos).

use std::collections::HashMap;

pub const NONE: usize = usize::MAX;
const INF: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub struct RegexError(pub String);

#[derive(Clone, Copy, Default, Debug)]
pub struct Flags {
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub unicode: bool,
    pub sticky: bool,
    pub has_indices: bool,
    pub unicode_sets: bool,
}

impl Flags {
    pub fn parse(s: &str) -> Option<Flags> {
        let mut f = Flags::default();
        for c in s.chars() {
            let slot = match c {
                'g' => &mut f.global,
                'i' => &mut f.ignore_case,
                'm' => &mut f.multiline,
                's' => &mut f.dot_all,
                'u' => &mut f.unicode,
                'y' => &mut f.sticky,
                'd' => &mut f.has_indices,
                'v' => &mut f.unicode_sets,
                _ => return None,
            };
            if *slot {
                return None;
            }
            *slot = true;
        }
        if f.unicode && f.unicode_sets {
            return None;
        }
        Some(f)
    }

    /// Modo Unicode (flag u ou v).
    pub fn full_unicode(&self) -> bool {
        self.unicode || self.unicode_sets
    }
}

// ---------------------------------------------------------------- classes

#[derive(Clone, Copy, Debug, PartialEq)]
enum Prop {
    Any,
    Ascii,
    AsciiHex,
    Alphabetic,
    Letter,
    Upper,
    Lower,
    Number,
    Digit,
    Punct,
    Symbol,
    Space,
    Control,
    Mark,
    Emoji,
    /// Faixas de uma escrita (Latin, Greek...).
    Script(&'static [(u32, u32)]),
}

#[derive(Clone, Debug, Default)]
pub struct ClassSet {
    /// Faixas ordenadas e sem sobreposição.
    ranges: Vec<(u32, u32)>,
    props: Vec<(Prop, bool)>,
    negated: bool,
}

const LATIN: &[(u32, u32)] = &[(0x41, 0x5A), (0x61, 0x7A), (0xAA, 0xAA), (0xBA, 0xBA), (0xC0, 0xD6), (0xD8, 0xF6), (0xF8, 0x24F), (0x1E00, 0x1EFF), (0x2C60, 0x2C7F), (0xA720, 0xA7FF), (0xFF21, 0xFF3A), (0xFF41, 0xFF5A)];
const GREEK: &[(u32, u32)] = &[(0x370, 0x3FF), (0x1F00, 0x1FFF)];
const CYRILLIC: &[(u32, u32)] = &[(0x400, 0x52F), (0x2DE0, 0x2DFF), (0xA640, 0xA69F)];
const HAN: &[(u32, u32)] = &[(0x2E80, 0x2FDF), (0x3005, 0x3007), (0x3021, 0x3029), (0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xF900, 0xFAFF), (0x20000, 0x3134F)];
const HIRAGANA: &[(u32, u32)] = &[(0x3041, 0x309F)];
const KATAKANA: &[(u32, u32)] = &[(0x30A0, 0x30FF), (0x31F0, 0x31FF), (0xFF66, 0xFF9F)];
const ARABIC: &[(u32, u32)] = &[(0x600, 0x6FF), (0x750, 0x77F), (0x8A0, 0x8FF), (0xFB50, 0xFDFF), (0xFE70, 0xFEFF)];
const HEBREW: &[(u32, u32)] = &[(0x591, 0x5F4), (0xFB1D, 0xFB4F)];
const HANGUL: &[(u32, u32)] = &[(0x1100, 0x11FF), (0x3130, 0x318F), (0xAC00, 0xD7AF)];
const THAI: &[(u32, u32)] = &[(0xE01, 0xE5B)];
const DEVANAGARI: &[(u32, u32)] = &[(0x900, 0x97F), (0xA8E0, 0xA8FF)];

fn is_emoji(c: u32) -> bool {
    matches!(c, 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F000..=0x1F2FF | 0x2300..=0x23FF | 0x2B00..=0x2BFF | 0x3030 | 0x303D | 0x3297 | 0x3299 | 0xA9 | 0xAE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x21AA)
}

fn is_punct(c: u32) -> bool {
    if c < 0x80 {
        return matches!(c, 0x21..=0x23 | 0x25..=0x2A | 0x2C..=0x2F | 0x3A | 0x3B | 0x3F | 0x40 | 0x5B..=0x5D | 0x5F | 0x7B | 0x7D);
    }
    matches!(c, 0xA1 | 0xA7 | 0xAB | 0xB6 | 0xB7 | 0xBB | 0xBF | 0x2010..=0x2027 | 0x2030..=0x205E | 0x3001..=0x3003 | 0x3008..=0x3011 | 0xFF01..=0xFF0F)
}

fn is_symbol(c: u32) -> bool {
    if c < 0x80 {
        return matches!(c, 0x24 | 0x2B | 0x3C..=0x3E | 0x5E | 0x60 | 0x7C | 0x7E);
    }
    matches!(c, 0xA2..=0xA6 | 0xA8 | 0xA9 | 0xAC | 0xAE..=0xB1 | 0xB4 | 0xD7 | 0xF7 | 0x2100..=0x214F | 0x2190..=0x23FF | 0x2500..=0x27BF | 0x20A0..=0x20CF) || is_emoji(c)
}

impl Prop {
    fn test(self, c: u32) -> bool {
        let ch = char::from_u32(c);
        match self {
            Prop::Any => true,
            Prop::Ascii => c < 0x80,
            Prop::AsciiHex => matches!(c, 0x30..=0x39 | 0x41..=0x46 | 0x61..=0x66),
            Prop::Alphabetic => ch.is_some_and(|c| c.is_alphabetic()),
            Prop::Letter => ch.is_some_and(|c| c.is_alphabetic() && !c.is_numeric()),
            Prop::Upper => ch.is_some_and(|c| c.is_uppercase()),
            Prop::Lower => ch.is_some_and(|c| c.is_lowercase()),
            Prop::Number => ch.is_some_and(|c| c.is_numeric()),
            Prop::Digit => ch.is_some_and(|c| c.is_numeric()),
            Prop::Punct => is_punct(c),
            Prop::Symbol => is_symbol(c),
            Prop::Space => ch.is_some_and(|c| c.is_whitespace()),
            Prop::Control => ch.is_some_and(|c| c.is_control()),
            Prop::Mark => matches!(c, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F),
            Prop::Emoji => is_emoji(c),
            Prop::Script(r) => r.iter().any(|&(a, b)| c >= a && c <= b),
        }
    }
}

fn prop_by_name(name: &str, value: Option<&str>) -> Option<Prop> {
    if let Some(v) = value {
        return match name {
            "General_Category" | "gc" => prop_by_name(v, None),
            "Script" | "sc" | "Script_Extensions" | "scx" => script(v),
            _ => None,
        };
    }
    Some(match name {
        "Any" => Prop::Any,
        "ASCII" => Prop::Ascii,
        "ASCII_Hex_Digit" | "AHex" | "Hex_Digit" => Prop::AsciiHex,
        "Alphabetic" | "Alpha" => Prop::Alphabetic,
        "L" | "Letter" | "LC" | "Cased_Letter" | "Lo" | "Other_Letter" | "Lm" | "Lt" | "ID_Start" | "IDS" | "XID_Start" => Prop::Letter,
        "Lu" | "Uppercase_Letter" | "Uppercase" | "Upper" => Prop::Upper,
        "Ll" | "Lowercase_Letter" | "Lowercase" | "Lower" => Prop::Lower,
        "N" | "Number" | "Nl" | "No" => Prop::Number,
        "Nd" | "Decimal_Number" | "digit" => Prop::Digit,
        "P" | "Punctuation" | "punct" | "Pc" | "Pd" | "Ps" | "Pe" | "Pi" | "Pf" | "Po" => Prop::Punct,
        "S" | "Symbol" | "Sm" | "Sc" | "Sk" | "So" => Prop::Symbol,
        "Z" | "Separator" | "Zs" | "Space_Separator" | "White_Space" | "space" | "Zl" | "Zp" => Prop::Space,
        "C" | "Other" | "Cc" | "Control" | "cntrl" => Prop::Control,
        "M" | "Mark" | "Combining_Mark" | "Mn" | "Mc" | "Me" => Prop::Mark,
        "Emoji" | "Emoji_Presentation" | "Extended_Pictographic" | "ExtPict" => Prop::Emoji,
        _ => return script(name),
    })
}

fn script(v: &str) -> Option<Prop> {
    Some(Prop::Script(match v {
        "Latin" | "Latn" => LATIN,
        "Greek" | "Grek" => GREEK,
        "Cyrillic" | "Cyrl" => CYRILLIC,
        "Han" | "Hani" => HAN,
        "Hiragana" | "Hira" => HIRAGANA,
        "Katakana" | "Kana" => KATAKANA,
        "Arabic" | "Arab" => ARABIC,
        "Hebrew" | "Hebr" => HEBREW,
        "Hangul" | "Hang" => HANGUL,
        "Thai" => THAI,
        "Devanagari" | "Deva" => DEVANAGARI,
        _ => return None,
    }))
}

const SPACE_RANGES: &[(u32, u32)] = &[
    (0x09, 0x0D),
    (0x20, 0x20),
    (0xA0, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
    (0xFEFF, 0xFEFF),
];
const DIGIT_RANGES: &[(u32, u32)] = &[(0x30, 0x39)];
const WORD_RANGES: &[(u32, u32)] = &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)];

impl ClassSet {
    fn add(&mut self, a: u32, b: u32) {
        self.ranges.push((a.min(b), a.max(b)));
    }

    fn add_all(&mut self, rs: &[(u32, u32)]) {
        self.ranges.extend_from_slice(rs);
    }

    /// Complemento de uma lista de faixas (para \D, \W, \S dentro de classes).
    fn add_complement(&mut self, rs: &[(u32, u32)]) {
        let mut prev = 0u32;
        for &(a, b) in rs {
            if a > prev {
                self.ranges.push((prev, a - 1));
            }
            prev = b + 1;
        }
        self.ranges.push((prev, 0x10FFFF));
    }

    fn normalize(&mut self) {
        self.ranges.sort();
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(self.ranges.len());
        for &(a, b) in &self.ranges {
            if let Some(last) = out.last_mut()
                && a <= last.1.saturating_add(1)
            {
                last.1 = last.1.max(b);
                continue;
            }
            out.push((a, b));
        }
        self.ranges = out;
    }

    fn contains_raw(&self, c: u32) -> bool {
        let r = &self.ranges;
        let i = r.partition_point(|&(_, b)| b < c);
        if i < r.len() && r[i].0 <= c {
            return true;
        }
        self.props.iter().any(|&(p, neg)| p.test(c) != neg)
    }

    fn matches(&self, c: u32, ignore_case: bool, unicode: bool) -> bool {
        let mut hit = self.contains_raw(c);
        if !hit && ignore_case {
            for v in case_variants(c) {
                if v != c && canon(v, unicode) == canon(c, unicode) && self.contains_raw(v) {
                    hit = true;
                    break;
                }
            }
        }
        hit != self.negated
    }
}

fn case_variants(c: u32) -> [u32; 3] {
    let Some(ch) = char::from_u32(c) else { return [c, c, c] };
    let single = |it: &mut dyn Iterator<Item = char>| -> u32 {
        match (it.next(), it.next()) {
            (Some(x), None) => x as u32,
            _ => c,
        }
    };
    let l = single(&mut ch.to_lowercase());
    let u = single(&mut ch.to_uppercase());
    let extra = match c {
        0x73 | 0x53 => 0x17F,
        0x17F => 0x73,
        0x6B | 0x4B => 0x212A,
        0x212A => 0x6B,
        _ => c,
    };
    [l, u, extra]
}

/// Canonicalize do spec (para comparar sem diferenciar maiúsculas).
pub fn canon(c: u32, unicode: bool) -> u32 {
    if c < 0x80 {
        if unicode {
            return if (0x41..=0x5A).contains(&c) { c + 32 } else { c };
        }
        return if (0x61..=0x7A).contains(&c) { c - 32 } else { c };
    }
    let Some(ch) = char::from_u32(c) else { return c };
    if unicode {
        match c {
            0x17F => return 0x73,
            0x212A => return 0x6B,
            _ => {}
        }
        let mut it = ch.to_lowercase();
        match (it.next(), it.next()) {
            (Some(l), None) => l as u32,
            _ => c,
        }
    } else {
        let mut it = ch.to_uppercase();
        match (it.next(), it.next()) {
            (Some(u), None) if (u as u32) >= 0x80 => u as u32,
            _ => c,
        }
    }
}

// ------------------------------------------------------------------ árvore

#[derive(Clone, Debug)]
enum Node {
    Empty,
    Char(u32),
    Any,
    Class(ClassSet),
    Start,
    End,
    WordB(bool),
    Group(Box<Node>, Option<u32>),
    Look { ahead: bool, negate: bool, node: Box<Node> },
    BackRef(u32),
    NamedRef(String),
    Repeat { node: Box<Node>, min: u32, max: u32, greedy: bool },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
}

struct Parser {
    s: Vec<u32>,
    i: usize,
    u: bool,
    /// Há grupos nomeados (então `\k` é referência mesmo sem a flag u).
    has_named: bool,
    total_groups: u32,
    ngroups: u32,
    names: Vec<(String, u32)>,
}

fn err<T>(msg: &str) -> Result<T, RegexError> {
    Err(RegexError(msg.into()))
}

fn is_syntax(c: u32) -> bool {
    matches!(char::from_u32(c), Some('^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'))
}

impl Parser {
    fn peek(&self) -> Option<u32> {
        self.s.get(self.i).copied()
    }

    fn peek_at(&self, k: usize) -> Option<u32> {
        self.s.get(self.i + k).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c as u32) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn is(&self, c: char) -> bool {
        self.peek() == Some(c as u32)
    }

    /// Conta grupos de captura e vê se há nomes (antes de analisar, para `\5` e `\k`).
    fn prescan(&mut self) {
        let mut i = 0;
        let mut in_class = false;
        while i < self.s.len() {
            let c = self.s[i];
            if c == '\\' as u32 {
                i += 2;
                continue;
            }
            if in_class {
                if c == ']' as u32 {
                    in_class = false;
                }
            } else if c == '[' as u32 {
                in_class = true;
            } else if c == '(' as u32 {
                if self.s.get(i + 1) != Some(&('?' as u32)) {
                    self.total_groups += 1;
                } else if self.s.get(i + 2) == Some(&('<' as u32))
                    && !matches!(self.s.get(i + 3).and_then(|&c| char::from_u32(c)), Some('=' | '!'))
                {
                    self.total_groups += 1;
                    self.has_named = true;
                }
            }
            i += 1;
        }
    }

    fn disjunction(&mut self) -> Result<Node, RegexError> {
        let mut alts = vec![self.alternative()?];
        while self.eat('|') {
            alts.push(self.alternative()?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) })
    }

    fn alternative(&mut self) -> Result<Node, RegexError> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' as u32 || c == ')' as u32 {
                break;
            }
            items.push(self.term()?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap(),
            _ => Node::Concat(items),
        })
    }

    fn term(&mut self) -> Result<Node, RegexError> {
        let c = self.peek().unwrap_or(0);
        // asserções
        match char::from_u32(c) {
            Some('^') => {
                self.i += 1;
                return Ok(Node::Start);
            }
            Some('$') => {
                self.i += 1;
                return Ok(Node::End);
            }
            Some('\\') if matches!(self.peek_at(1).and_then(char::from_u32), Some('b' | 'B')) => {
                let neg = self.peek_at(1) == Some('B' as u32);
                self.i += 2;
                return Ok(Node::WordB(neg));
            }
            Some('(') if self.peek_at(1) == Some('?' as u32) => {
                let k = self.peek_at(2).and_then(char::from_u32);
                let k3 = self.peek_at(3).and_then(char::from_u32);
                let look = match (k, k3) {
                    (Some('='), _) => Some((true, false, 3)),
                    (Some('!'), _) => Some((true, true, 3)),
                    (Some('<'), Some('=')) => Some((false, false, 4)),
                    (Some('<'), Some('!')) => Some((false, true, 4)),
                    _ => None,
                };
                if let Some((ahead, negate, skip)) = look {
                    self.i += skip;
                    let node = self.disjunction()?;
                    if !self.eat(')') {
                        return err("Unterminated group");
                    }
                    let n = Node::Look {
                        ahead,
                        negate,
                        node: Box::new(node),
                    };
                    // Anexo B: lookahead pode ter quantificador sem a flag u
                    if ahead && !self.u && self.at_quantifier() {
                        return self.quantifier(n);
                    }
                    return Ok(n);
                }
            }
            _ => {}
        }
        let atom = self.atom()?;
        if self.at_quantifier() {
            return self.quantifier(atom);
        }
        Ok(atom)
    }

    fn at_quantifier(&self) -> bool {
        match self.peek().and_then(char::from_u32) {
            Some('*' | '+' | '?') => true,
            Some('{') => self.brace_quantifier().is_some(),
            _ => false,
        }
    }

    /// `{n}`, `{n,}`, `{n,m}` a partir da posição atual: (min, max, tamanho).
    fn brace_quantifier(&self) -> Option<(u32, u32, usize)> {
        let mut j = self.i + 1;
        let num = |j: &mut usize| -> Option<u32> {
            let start = *j;
            let mut v: u64 = 0;
            while let Some(&c) = self.s.get(*j) {
                if !(0x30..=0x39).contains(&c) {
                    break;
                }
                v = (v * 10 + (c - 0x30) as u64).min(INF as u64);
                *j += 1;
            }
            if *j == start { None } else { Some(v as u32) }
        };
        let min = num(&mut j)?;
        let max = if self.s.get(j) == Some(&(',' as u32)) {
            j += 1;
            if self.s.get(j) == Some(&('}' as u32)) { INF } else { num(&mut j)? }
        } else {
            min
        };
        if self.s.get(j) != Some(&('}' as u32)) {
            return None;
        }
        Some((min, max, j + 1 - self.i))
    }

    fn quantifier(&mut self, atom: Node) -> Result<Node, RegexError> {
        let (min, max) = match char::from_u32(self.peek().unwrap_or(0)) {
            Some('*') => {
                self.i += 1;
                (0, INF)
            }
            Some('+') => {
                self.i += 1;
                (1, INF)
            }
            Some('?') => {
                self.i += 1;
                (0, 1)
            }
            _ => {
                let (a, b, len) = self.brace_quantifier().ok_or(RegexError("Nothing to repeat".into()))?;
                if b < a {
                    return err("numbers out of order in {} quantifier");
                }
                self.i += len;
                (a, b)
            }
        };
        let greedy = !self.eat('?');
        if matches!(atom, Node::Start | Node::End | Node::WordB(_)) || matches!(atom, Node::Look { ahead: false, .. }) {
            return err("Nothing to repeat");
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            greedy,
        })
    }

    fn group_name(&mut self) -> Result<String, RegexError> {
        let mut name = String::new();
        loop {
            let Some(c) = self.peek() else { return err("Invalid capture group name") };
            self.i += 1;
            if c == '>' as u32 {
                break;
            }
            let ch = if c == '\\' as u32 && self.eat('u') {
                self.unicode_escape_value(true)?.ok_or(RegexError("Invalid capture group name".into()))?
            } else {
                c
            };
            let ch = char::from_u32(ch).ok_or(RegexError("Invalid capture group name".into()))?;
            let ok = if name.is_empty() { ch.is_alphabetic() || ch == '$' || ch == '_' } else { ch.is_alphanumeric() || ch == '$' || ch == '_' || ch == '\u{200C}' || ch == '\u{200D}' };
            if !ok {
                return err("Invalid capture group name");
            }
            name.push(ch);
        }
        if name.is_empty() {
            return err("Invalid capture group name");
        }
        Ok(name)
    }

    fn atom(&mut self) -> Result<Node, RegexError> {
        let c = self.peek().ok_or(RegexError("Unexpected end".into()))?;
        match char::from_u32(c) {
            Some('.') => {
                self.i += 1;
                Ok(Node::Any)
            }
            Some('(') => {
                self.i += 1;
                if self.eat('?') {
                    if self.eat(':') {
                        let d = self.disjunction()?;
                        if !self.eat(')') {
                            return err("Unterminated group");
                        }
                        return Ok(Node::Group(Box::new(d), None));
                    }
                    if self.eat('<') {
                        let name = self.group_name()?;
                        if self.names.iter().any(|(n, _)| *n == name) {
                            return err("Duplicate capture group name");
                        }
                        self.ngroups += 1;
                        let idx = self.ngroups;
                        self.names.push((name, idx));
                        let d = self.disjunction()?;
                        if !self.eat(')') {
                            return err("Unterminated group");
                        }
                        return Ok(Node::Group(Box::new(d), Some(idx)));
                    }
                    return err("Invalid group");
                }
                self.ngroups += 1;
                let idx = self.ngroups;
                let d = self.disjunction()?;
                if !self.eat(')') {
                    return err("Unterminated group");
                }
                Ok(Node::Group(Box::new(d), Some(idx)))
            }
            Some(')') => err("Unmatched ')'"),
            Some('[') => {
                self.i += 1;
                Ok(Node::Class(self.class()?))
            }
            Some('*' | '+' | '?') => err("Nothing to repeat"),
            Some('{') => {
                if self.u || self.brace_quantifier().is_some() {
                    return err(if self.u { "Lone quantifier brackets" } else { "Nothing to repeat" });
                }
                self.i += 1;
                Ok(Node::Char(c))
            }
            Some('}' | ']') if self.u => err("Lone quantifier brackets"),
            Some('\\') => {
                self.i += 1;
                self.atom_escape()
            }
            _ => {
                self.i += 1;
                Ok(Node::Char(c))
            }
        }
    }

    fn class_escape_set(&self, c: char) -> Option<ClassSet> {
        let mut set = ClassSet::default();
        match c {
            'd' => set.add_all(DIGIT_RANGES),
            'D' => set.add_complement(DIGIT_RANGES),
            's' => set.add_all(SPACE_RANGES),
            'S' => set.add_complement(SPACE_RANGES),
            'w' => set.add_all(WORD_RANGES),
            'W' => set.add_complement(WORD_RANGES),
            _ => return None,
        }
        Some(set)
    }

    fn property(&mut self) -> Result<(Prop, bool), RegexError> {
        let negate = self.s[self.i - 1] == 'P' as u32;
        if !self.eat('{') {
            return err("Invalid property name");
        }
        let mut text = String::new();
        loop {
            let Some(c) = self.peek() else { return err("Invalid property name") };
            self.i += 1;
            if c == '}' as u32 {
                break;
            }
            text.push(char::from_u32(c).unwrap_or('?'));
        }
        let (name, value) = match text.split_once('=') {
            Some((a, b)) => (a, Some(b)),
            None => (text.as_str(), None),
        };
        let p = prop_by_name(name, value).ok_or(RegexError("Invalid property name".into()))?;
        Ok((p, negate))
    }

    fn hex(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for k in 0..n {
            let d = char::from_u32(self.peek_at(k)?)?.to_digit(16)?;
            v = v * 16 + d;
        }
        self.i += n;
        Some(v)
    }

    /// Depois de `\u`: `XXXX`, par substituto, ou `{...}` (modo u).
    fn unicode_escape_value(&mut self, allow_brace: bool) -> Result<Option<u32>, RegexError> {
        if (self.u || allow_brace) && self.eat('{') {
            let start = self.i;
            let mut v: u32 = 0;
            while let Some(d) = self.peek().and_then(char::from_u32).and_then(|c| c.to_digit(16)) {
                v = v.saturating_mul(16).saturating_add(d);
                self.i += 1;
            }
            if self.i == start || !self.eat('}') || v > 0x10FFFF {
                return err("Invalid Unicode escape");
            }
            return Ok(Some(v));
        }
        let Some(v) = self.hex(4) else { return Ok(None) };
        if self.u
            && (0xD800..0xDC00).contains(&v)
            && self.peek() == Some('\\' as u32)
            && self.peek_at(1) == Some('u' as u32)
        {
            let save = self.i;
            self.i += 2;
            match self.hex(4) {
                Some(lo) if (0xDC00..0xE000).contains(&lo) => {
                    return Ok(Some(0x10000 + ((v - 0xD800) << 10) + (lo - 0xDC00)));
                }
                _ => self.i = save,
            }
        }
        Ok(Some(v))
    }

    /// Escapes de caractere comuns a átomos e classes. None = não é desse tipo.
    fn char_escape(&mut self, c: char, in_class: bool) -> Result<Option<u32>, RegexError> {
        Ok(Some(match c {
            'f' => 0x0C,
            'n' => 0x0A,
            'r' => 0x0D,
            't' => 0x09,
            'v' => 0x0B,
            'c' => {
                let next = self.peek().and_then(char::from_u32);
                match next {
                    Some(l) if l.is_ascii_alphabetic() => {
                        self.i += 1;
                        (l as u32) % 32
                    }
                    Some(d) if in_class && !self.u && (d.is_ascii_digit() || d == '_') => {
                        self.i += 1;
                        (d as u32) % 32
                    }
                    _ => {
                        if self.u {
                            return err("Invalid unicode escape");
                        }
                        // Anexo B: "\c" vira barra invertida literal seguida de "c"
                        self.i -= 1;
                        0x5C
                    }
                }
            }
            'x' => match self.hex(2) {
                Some(v) => v,
                None if self.u => return err("Invalid escape"),
                None => 'x' as u32,
            },
            'u' => match self.unicode_escape_value(false)? {
                Some(v) => v,
                None if self.u => return err("Invalid Unicode escape"),
                None => 'u' as u32,
            },
            '0' if !self.peek().is_some_and(|d| (0x30..=0x39).contains(&d)) => 0,
            _ => return Ok(None),
        }))
    }

    /// Escape octal legado (Anexo B): até três dígitos, valor ≤ 0o377.
    fn legacy_octal(&mut self, first: u32) -> u32 {
        let mut v = first - 0x30;
        for _ in 0..2 {
            match self.peek() {
                Some(d) if (0x30..=0x37).contains(&d) && v * 8 + (d - 0x30) <= 0o377 => {
                    v = v * 8 + (d - 0x30);
                    self.i += 1;
                }
                _ => break,
            }
        }
        v
    }

    fn atom_escape(&mut self) -> Result<Node, RegexError> {
        let Some(c) = self.peek() else { return err("\\ at end of pattern") };
        self.i += 1;
        let ch = char::from_u32(c).unwrap_or('\0');
        if let Some(set) = self.class_escape_set(ch) {
            return Ok(Node::Class(set));
        }
        if (ch == 'p' || ch == 'P') && self.u {
            let (p, neg) = self.property()?;
            let mut set = ClassSet::default();
            set.props.push((p, neg));
            return Ok(Node::Class(set));
        }
        if ch == 'k' && (self.u || self.has_named) {
            if !self.eat('<') {
                return err("Invalid named reference");
            }
            let name = self.group_name()?;
            return Ok(Node::NamedRef(name));
        }
        if ('1'..='9').contains(&ch) {
            // referência \N, ou octal/identidade legada
            let start = self.i - 1;
            let mut n: u32 = c - 0x30;
            while let Some(d) = self.peek().filter(|d| (0x30..=0x39).contains(d)) {
                n = n.saturating_mul(10).saturating_add(d - 0x30);
                self.i += 1;
            }
            if n <= self.total_groups {
                return Ok(Node::BackRef(n));
            }
            if self.u {
                return err("Invalid escape");
            }
            self.i = start + 1;
            if ch >= '8' {
                return Ok(Node::Char(c));
            }
            return Ok(Node::Char(self.legacy_octal(c)));
        }
        if ch == '0' && self.peek().is_some_and(|d| (0x30..=0x39).contains(&d)) {
            if self.u {
                return err("Invalid decimal escape");
            }
            return Ok(Node::Char(self.legacy_octal(c)));
        }
        if let Some(v) = self.char_escape(ch, false)? {
            return Ok(Node::Char(v));
        }
        if self.u && !is_syntax(c) && ch != '/' && ch != '-' {
            return err("Invalid escape");
        }
        Ok(Node::Char(c))
    }

    fn class(&mut self) -> Result<ClassSet, RegexError> {
        let mut set = ClassSet::default();
        if self.eat('^') {
            set.negated = true;
        }
        loop {
            let Some(c) = self.peek() else { return err("Unterminated character class") };
            if c == ']' as u32 {
                self.i += 1;
                break;
            }
            let a = self.class_atom(&mut set)?;
            if self.is('-') && self.peek_at(1).is_some_and(|x| x != ']' as u32) {
                let save = self.i;
                self.i += 1;
                let mut tmp = ClassSet::default();
                let b = self.class_atom(&mut tmp)?;
                match (a, b) {
                    (Some(a), Some(b)) => {
                        if a > b {
                            return err("Range out of order in character class");
                        }
                        set.add(a, b);
                    }
                    _ => {
                        if self.u {
                            return err("Invalid character class");
                        }
                        // Anexo B: \d-x vira \d, '-', x
                        if let Some(a) = a {
                            set.add(a, a);
                        }
                        set.add('-' as u32, '-' as u32);
                        if let Some(b) = b {
                            set.add(b, b);
                        }
                        set.ranges.extend(tmp.ranges);
                        set.props.extend(tmp.props);
                    }
                }
                let _ = save;
            } else if let Some(a) = a {
                set.add(a, a);
            }
        }
        set.normalize();
        Ok(set)
    }

