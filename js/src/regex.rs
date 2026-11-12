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

    /// Um elemento de classe: caractere (Some) ou conjunto já adicionado (None).
    fn class_atom(&mut self, set: &mut ClassSet) -> Result<Option<u32>, RegexError> {
        let c = self.peek().ok_or(RegexError("Unterminated character class".into()))?;
        self.i += 1;
        if c != '\\' as u32 {
            // par substituto literal no modo u
            if self.u && (0xD800..0xDC00).contains(&c)
                && let Some(lo) = self.peek().filter(|l| (0xDC00..0xE000).contains(l))
            {
                self.i += 1;
                return Ok(Some(0x10000 + ((c - 0xD800) << 10) + (lo - 0xDC00)));
            }
            return Ok(Some(c));
        }
        let Some(e) = self.peek() else { return err("\\ at end of pattern") };
        self.i += 1;
        let ch = char::from_u32(e).unwrap_or('\0');
        if let Some(s) = self.class_escape_set(ch) {
            set.ranges.extend(s.ranges);
            return Ok(None);
        }
        if (ch == 'p' || ch == 'P') && self.u {
            let p = self.property()?;
            set.props.push(p);
            return Ok(None);
        }
        if ch == 'b' {
            return Ok(Some(0x08));
        }
        if ch == '-' && self.u {
            return Ok(Some('-' as u32));
        }
        if ch.is_ascii_digit() && !self.u && ch != '0' {
            if ch >= '8' {
                return Ok(Some(e));
            }
            return Ok(Some(self.legacy_octal(e)));
        }
        if ch == '0' && self.peek().is_some_and(|d| (0x30..=0x39).contains(&d)) && !self.u {
            return Ok(Some(self.legacy_octal(e)));
        }
        if let Some(v) = self.char_escape(ch, true)? {
            return Ok(Some(v));
        }
        if self.u && !is_syntax(e) && ch != '/' && ch != '-' {
            return err("Invalid escape");
        }
        Ok(Some(e))
    }
}

// --------------------------------------------------------------- bytecode

#[derive(Clone, Copy, Debug)]
enum Single {
    Char(u32),
    Any,
    Class(u32),
}

#[derive(Clone, Copy, Debug)]
enum Inst {
    Single { m: Single, back: bool },
    Repeat { m: Single, min: u32, max: u32, greedy: bool, back: bool },
    Split(u32, u32),
    Jmp(u32),
    Save(u32),
    Clear(u32, u32),
    Start,
    End,
    WordB(bool),
    BackRef { group: u32, back: bool },
    Look { negate: bool, end: u32 },
    SetReg(u32),
    EmptyCheck(u32),
    /// Falha se a iteração foi vazia e o mínimo já foi atingido.
    EmptyCheckMin { pos: u32, count: u32, min: u32 },
    CountInit(u32),
    CountInc(u32),
    CountLt { r: u32, n: u32, target: u32 },
    CountGe { r: u32, n: u32, target: u32 },
    Succeed,
    Match,
}

pub struct Program {
    insts: Vec<Inst>,
    classes: Vec<ClassSet>,
    /// Grupos de captura (sem contar o 0).
    pub ngroups: u32,
    nregs: u32,
    pub names: Vec<(String, u32)>,
    pub flags: Flags,
    /// Unidades com que o casamento pode começar (acelera a busca).
    first: Option<FirstSet>,
    /// Começa com `^` sem a flag m: só a posição 0 pode casar.
    anchored: bool,
}

/// Conjunto de unidades UTF-16 iniciais possíveis.
enum FirstSet {
    One(u16),
    Ranges(Vec<(u16, u16)>),
}

impl FirstSet {
    fn find(&self, s: &[u16], from: usize) -> Option<usize> {
        match self {
            FirstSet::One(c) => s[from..].iter().position(|x| x == c).map(|k| from + k),
            FirstSet::Ranges(r) => s[from..]
                .iter()
                .position(|&x| r.iter().any(|&(a, b)| x >= a && x <= b))
                .map(|k| from + k),
        }
    }
}

struct Compiler {
    insts: Vec<Inst>,
    classes: Vec<ClassSet>,
    nregs: u32,
    flags: Flags,
    names: HashMap<String, u32>,
}

fn group_range(n: &Node, out: &mut Option<(u32, u32)>) {
    match n {
        Node::Group(inner, idx) => {
            if let Some(i) = idx {
                *out = Some(match *out {
                    Some((a, b)) => (a.min(*i), b.max(*i)),
                    None => (*i, *i),
                });
            }
            group_range(inner, out);
        }
        Node::Look { node, .. } => group_range(node, out),
        Node::Repeat { node, .. } => group_range(node, out),
        Node::Concat(v) | Node::Alt(v) => v.iter().for_each(|x| group_range(x, out)),
        _ => {}
    }
}

impl Compiler {
    fn emit(&mut self, i: Inst) -> u32 {
        self.insts.push(i);
        (self.insts.len() - 1) as u32
    }

    fn here(&self) -> u32 {
        self.insts.len() as u32
    }

    fn reg(&mut self) -> u32 {
        let r = self.nregs;
        self.nregs += 1;
        r
    }

    fn single(&mut self, n: &Node) -> Option<Single> {
        Some(match n {
            Node::Char(c) => Single::Char(*c),
            Node::Any => Single::Any,
            Node::Class(cs) => {
                self.classes.push(cs.clone());
                Single::Class((self.classes.len() - 1) as u32)
            }
            _ => return None,
        })
    }

    fn set_target(&mut self, at: u32, t: u32) {
        match &mut self.insts[at as usize] {
            Inst::Split(_, b) => *b = t,
            Inst::Jmp(x) => *x = t,
            Inst::CountLt { target, .. } | Inst::CountGe { target, .. } => *target = t,
            Inst::Look { end, .. } => *end = t,
            _ => {}
        }
    }

    fn compile(&mut self, n: &Node, back: bool) -> Result<(), RegexError> {
        match n {
            Node::Empty => {}
            Node::Char(_) | Node::Any | Node::Class(_) => {
                if let Node::Char(c) = n
                    && *c > 0xFFFF
                    && !self.flags.full_unicode()
                {
                    // sem a flag u, um caractere astral são duas unidades
                    let c = c - 0x10000;
                    let (hi, lo) = (0xD800 + (c >> 10), 0xDC00 + (c & 0x3FF));
                    let order = if back { [lo, hi] } else { [hi, lo] };
                    for u in order {
                        self.emit(Inst::Single { m: Single::Char(u), back });
                    }
                    return Ok(());
                }
                let m = self.single(n).unwrap();
                self.emit(Inst::Single { m, back });
            }
            Node::Start => {
                self.emit(Inst::Start);
            }
            Node::End => {
                self.emit(Inst::End);
            }
            Node::WordB(neg) => {
                self.emit(Inst::WordB(*neg));
            }
            Node::Group(inner, idx) => match idx {
                Some(i) => {
                    let (s, e) = (2 * i, 2 * i + 1);
                    self.emit(Inst::Save(if back { e } else { s }));
                    self.compile(inner, back)?;
                    self.emit(Inst::Save(if back { s } else { e }));
                }
                None => self.compile(inner, back)?,
            },
            Node::BackRef(g) => {
                self.emit(Inst::BackRef { group: *g, back });
            }
            Node::NamedRef(name) => {
                let g = *self.names.get(name).ok_or(RegexError("Invalid named capture referenced".into()))?;
                self.emit(Inst::BackRef { group: g, back });
            }
            Node::Look { ahead, negate, node } => {
                let at = self.emit(Inst::Look { negate: *negate, end: 0 });
                self.compile(node, !*ahead)?;
                self.emit(Inst::Succeed);
                let end = self.here();
                self.set_target(at, end);
            }
            Node::Concat(items) => {
                if back {
                    for it in items.iter().rev() {
                        self.compile(it, back)?;
                    }
                } else {
                    for it in items {
                        self.compile(it, back)?;
                    }
                }
            }
            Node::Alt(alts) => {
                let mut jumps = Vec::new();
                for (k, a) in alts.iter().enumerate() {
                    if k + 1 < alts.len() {
                        let split = self.emit(Inst::Split(0, 0));
                        let first = self.here();
                        if let Inst::Split(x, _) = &mut self.insts[split as usize] {
                            *x = first;
                        }
                        self.compile(a, back)?;
                        jumps.push(self.emit(Inst::Jmp(0)));
                        let next = self.here();
                        self.set_target(split, next);
                    } else {
                        self.compile(a, back)?;
                    }
                }
                let end = self.here();
                for j in jumps {
                    self.set_target(j, end);
                }
            }
            Node::Repeat { node, min, max, greedy } => self.repeat(node, *min, *max, *greedy, back)?,
        }
        Ok(())
    }

    fn split(&mut self, greedy: bool, body: u32, exit: u32) -> Inst {
        if greedy { Inst::Split(body, exit) } else { Inst::Split(exit, body) }
    }

    fn repeat(&mut self, node: &Node, min: u32, max: u32, greedy: bool, back: bool) -> Result<(), RegexError> {
        if max == 0 {
            return Ok(());
        }
        // um caractere só, sem grupos: instrução própria
        let simple = matches!(node, Node::Any | Node::Class(_)) || matches!(node, Node::Char(c) if *c <= 0xFFFF || self.flags.full_unicode());
        if simple {
            let m = self.single(node).unwrap();
            self.emit(Inst::Repeat { m, min, max, greedy, back });
            return Ok(());
        }
        let mut caps = None;
        group_range(node, &mut caps);
        let clear = |c: &mut Compiler| {
            if let Some((a, b)) = caps {
                c.emit(Inst::Clear(2 * a, 2 * b + 1));
            }
        };
        if min == 0 && max == 1 {
            let split = self.emit(Inst::Split(0, 0));
            let body = self.here();
            clear(self);
            self.compile(node, back)?;
            let exit = self.here();
            self.insts[split as usize] = self.split(greedy, body, exit);
            return Ok(());
        }
        if max == INF && min <= 1 {
            if min == 1 {
                clear(self);
                self.compile(node, back)?;
            }
            let p = self.reg();
            let top = self.emit(Inst::Split(0, 0));
            let body = self.here();
            self.emit(Inst::SetReg(p));
            clear(self);
            self.compile(node, back)?;
            self.emit(Inst::EmptyCheck(p));
            self.emit(Inst::Jmp(top));
            let exit = self.here();
            self.insts[top as usize] = self.split(greedy, body, exit);
            return Ok(());
        }
        // caso geral com contador
        let c = self.reg();
        let p = self.reg();
        self.emit(Inst::CountInit(c));
        let top = self.here();
        let forced = self.emit(Inst::CountLt { r: c, n: min, target: 0 });
        let at_max = if max != INF { Some(self.emit(Inst::CountGe { r: c, n: max, target: 0 })) } else { None };
        let split = self.emit(Inst::Split(0, 0));
        let body = self.here();
        self.emit(Inst::SetReg(p));
        clear(self);
        self.compile(node, back)?;
        self.emit(Inst::EmptyCheckMin { pos: p, count: c, min });
        self.emit(Inst::CountInc(c));
        self.emit(Inst::Jmp(top));
        let forced_body = self.here();
        self.set_target(forced, forced_body);
        clear(self);
        self.compile(node, back)?;
        self.emit(Inst::CountInc(c));
        self.emit(Inst::Jmp(top));
        let exit = self.here();
        self.insts[split as usize] = self.split(greedy, body, exit);
        if let Some(m) = at_max {
            self.set_target(m, exit);
        }
        Ok(())
    }
}

/// Faixas de unidades com que o nó pode começar (None = qualquer uma / vazio).
fn first_ranges(n: &Node, icase: bool, unicode: bool) -> Option<Vec<(u32, u32)>> {
    match n {
        Node::Char(c) => {
            if *c > 0xFFFF {
                let hi = 0xD800 + ((c - 0x10000) >> 10);
                return Some(vec![(hi, hi)]);
            }
            let mut v = vec![(*c, *c)];
            if icase {
                for x in case_variants(*c) {
                    if canon(x, unicode) == canon(*c, unicode) && x <= 0xFFFF {
                        v.push((x, x));
                    }
                }
            }
            Some(v)
        }
        Node::Class(cs) if !cs.negated && cs.props.is_empty() && !icase => {
            let mut v = Vec::new();
            for &(a, b) in &cs.ranges {
                if b > 0xFFFF {
                    // astrais começam por um substituto alto
                    v.push((a.min(0xD800), 0xDBFF.max(b.min(0xFFFF))));
                } else {
                    v.push((a, b));
                }
            }
            if v.is_empty() { None } else { Some(v) }
        }
        Node::Concat(items) => items.first().and_then(|x| first_ranges(x, icase, unicode)),
        Node::Group(inner, _) => first_ranges(inner, icase, unicode),
        Node::Repeat { node, min, .. } if *min > 0 => first_ranges(node, icase, unicode),
        Node::Alt(alts) => {
            let mut v = Vec::new();
            for a in alts {
                v.extend(first_ranges(a, icase, unicode)?);
            }
            Some(v)
        }
        _ => None,
    }
}

fn first_set(n: &Node, flags: Flags) -> Option<FirstSet> {
    let r = first_ranges(n, flags.ignore_case, flags.full_unicode())?;
    if r.len() == 1 && r[0].0 == r[0].1 {
        return Some(FirstSet::One(r[0].0 as u16));
    }
    if r.len() > 64 {
        return None;
    }
    Some(FirstSet::Ranges(r.into_iter().map(|(a, b)| (a as u16, b.min(0xFFFF) as u16)).collect()))
}

fn starts_anchored(n: &Node) -> bool {
    match n {
        Node::Start => true,
        Node::Concat(items) => items.first().is_some_and(starts_anchored),
        Node::Group(inner, _) => starts_anchored(inner),
        Node::Alt(alts) => alts.iter().all(starts_anchored),
        _ => false,
    }
}

/// Analisa e compila um padrão.
pub fn compile(pattern: &[u16], flags: Flags) -> Result<Program, RegexError> {
    // padrão como pontos de código (pares substitutos viram um só)
    let mut s = Vec::with_capacity(pattern.len());
    let mut i = 0;
    while i < pattern.len() {
        let c = pattern[i] as u32;
        if (0xD800..0xDC00).contains(&c) && i + 1 < pattern.len() && (0xDC00..0xE000).contains(&(pattern[i + 1] as u32)) {
            s.push(0x10000 + ((c - 0xD800) << 10) + (pattern[i + 1] as u32 - 0xDC00));
            i += 2;
        } else {
            s.push(c);
            i += 1;
        }
    }
    let mut p = Parser {
        s,
        i: 0,
        u: flags.full_unicode(),
        has_named: false,
        total_groups: 0,
        ngroups: 0,
        names: Vec::new(),
    };
    p.prescan();
    let tree = p.disjunction()?;
    if p.i < p.s.len() {
        return err(if p.s[p.i] == ')' as u32 { "Unmatched ')'" } else { "Unexpected character" });
    }
    let mut c = Compiler {
        insts: Vec::new(),
        classes: Vec::new(),
        nregs: 2 * (p.ngroups + 1),
        flags,
        names: p.names.iter().cloned().collect(),
    };
    c.emit(Inst::Save(0));
    c.compile(&tree, false)?;
    c.emit(Inst::Save(1));
    c.emit(Inst::Match);
    Ok(Program {
        insts: c.insts,
        classes: c.classes,
        ngroups: p.ngroups,
        nregs: c.nregs,
        names: p.names,
        flags,
        first: first_set(&tree, flags),
        anchored: !flags.multiline && starts_anchored(&tree),
    })
}

// ---------------------------------------------------------------- execução

enum Bt {
    Branch { pc: u32, pos: usize },
    Restore { reg: u32, val: usize },
    Greedy { pc: u32, min_pos: usize, cur: usize, back: bool },
    Lazy { pc: u32, rep: u32, cur: usize, count: u32 },
}

fn is_line_term(c: u16) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

struct Matcher<'a> {
    prog: &'a Program,
    s: &'a [u16],
    unicode: bool,
    icase: bool,
    steps: u64,
    /// Pilhas de retrocesso reaproveitadas entre tentativas.
    pool: Vec<Vec<Bt>>,
}

impl Matcher<'_> {
    fn read_fwd(&self, pos: usize) -> Option<(u32, usize)> {
        let c = *self.s.get(pos)? as u32;
        if self.unicode && (0xD800..0xDC00).contains(&c)
            && let Some(&lo) = self.s.get(pos + 1)
            && (0xDC00..0xE000).contains(&(lo as u32))
        {
            return Some((0x10000 + ((c - 0xD800) << 10) + (lo as u32 - 0xDC00), pos + 2));
        }
        Some((c, pos + 1))
    }

    fn read_back(&self, pos: usize) -> Option<(u32, usize)> {
        if pos == 0 {
            return None;
        }
        let c = self.s[pos - 1] as u32;
        if self.unicode && (0xDC00..0xE000).contains(&c) && pos >= 2 {
            let hi = self.s[pos - 2] as u32;
            if (0xD800..0xDC00).contains(&hi) {
                return Some((0x10000 + ((hi - 0xD800) << 10) + (c - 0xDC00), pos - 2));
            }
        }
        Some((c, pos - 1))
    }

    fn single_ok(&self, m: Single, c: u32) -> bool {
        match m {
            Single::Char(x) => c == x || (self.icase && canon(c, self.unicode) == canon(x, self.unicode)),
            Single::Any => self.prog.flags.dot_all || !(c <= 0xFFFF && is_line_term(c as u16)),
            Single::Class(i) => self.prog.classes[i as usize].matches(c, self.icase, self.unicode),
        }
    }

    /// Consome um caractere que casa `m` (para frente ou para trás).
    fn step(&self, m: Single, pos: usize, back: bool) -> Option<usize> {
        let (c, np) = if back { self.read_back(pos)? } else { self.read_fwd(pos)? };
        if self.single_ok(m, c) { Some(np) } else { None }
    }

    fn is_word(&self, pos: usize) -> bool {
        let Some(&c) = self.s.get(pos) else { return false };
        let c = c as u32;
        matches!(c, 0x30..=0x39 | 0x41..=0x5A | 0x5F | 0x61..=0x7A)
            || (self.unicode && self.icase && (c == 0x17F || c == 0x212A))
    }

    fn set(&self, regs: &mut [usize], stack: &mut Vec<Bt>, r: u32, v: usize) {
        let old = regs[r as usize];
        if old != v {
            stack.push(Bt::Restore { reg: r, val: old });
            regs[r as usize] = v;
        }
    }

    /// Roda o programa a partir de `pc` na posição `pos`. Devolve a posição final.
    fn run(&mut self, pc0: u32, pos0: usize, regs: &mut [usize]) -> Option<usize> {
        let mut stack = self.pool.pop().unwrap_or_default();
        stack.clear();
        let r = self.run_with(pc0, pos0, regs, &mut stack);
        stack.clear();
        self.pool.push(stack);
        r
    }

    fn run_with(&mut self, pc0: u32, pos0: usize, regs: &mut [usize], stack: &mut Vec<Bt>) -> Option<usize> {
        let insts = &self.prog.insts;
        let mut pc = pc0 as usize;
        let mut pos = pos0;
        loop {
            self.steps += 1;
            let ok: bool = match insts[pc] {
                Inst::Single { m, back } => match self.step(m, pos, back) {
                    Some(np) => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    None => false,
                },
                Inst::Repeat { m, min, max, greedy, back } => {
                    let mut count = 0u32;
                    let mut p = pos;
                    let mut ok = true;
                    while count < min {
                        match self.step(m, p, back) {
                            Some(np) => {
                                p = np;
                                count += 1;
                            }
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        if greedy {
                            let min_pos = p;
                            while count < max {
                                match self.step(m, p, back) {
                                    Some(np) => {
                                        p = np;
                                        count += 1;
                                    }
                                    None => break,
                                }
                            }
                            if p != min_pos {
                                stack.push(Bt::Greedy {
                                    pc: pc as u32 + 1,
                                    min_pos,
                                    cur: p,
                                    back,
                                });
                            }
                        } else if count < max {
                            stack.push(Bt::Lazy {
                                pc: pc as u32 + 1,
                                rep: pc as u32,
                                cur: p,
                                count,
                            });
                        }
                        pos = p;
                        pc += 1;
                    }
                    ok
                }
                Inst::Split(a, b) => {
                    stack.push(Bt::Branch { pc: b, pos });
                    pc = a as usize;
                    true
                }
                Inst::Jmp(t) => {
                    pc = t as usize;
                    true
                }
                Inst::Save(slot) => {
                    self.set(regs, stack, slot, pos);
                    pc += 1;
                    true
                }
                Inst::Clear(a, b) => {
                    for r in a..=b {
                        self.set(regs, stack, r, NONE);
                    }
                    pc += 1;
                    true
                }
                Inst::Start => {
                    let ok = pos == 0 || (self.prog.flags.multiline && is_line_term(self.s[pos - 1]));
                    pc += 1;
                    ok
                }
                Inst::End => {
                    let ok = pos == self.s.len() || (self.prog.flags.multiline && is_line_term(self.s[pos]));
                    pc += 1;
                    ok
                }
                Inst::WordB(neg) => {
                    let a = pos > 0 && self.is_word(pos - 1);
                    let b = self.is_word(pos);
                    pc += 1;
                    (a != b) != neg
                }
                Inst::BackRef { group, back } => {
                    let (s, e) = (regs[2 * group as usize], regs[2 * group as usize + 1]);
                    pc += 1;
                    if s == NONE || e == NONE || e < s {
                        true
                    } else {
                        let len = e - s;
                        let start = if back {
                            if pos < len {
                                usize::MAX
                            } else {
                                pos - len
                            }
                        } else {
                            pos
                        };
                        if start == usize::MAX || start + len > self.s.len() {
                            false
                        } else {
                            let ok = (0..len).all(|k| {
                                let (x, y) = (self.s[s + k] as u32, self.s[start + k] as u32);
                                x == y || (self.icase && canon(x, self.unicode) == canon(y, self.unicode))
                            });
                            if ok {
                                pos = if back { start } else { pos + len };
                            }
                            ok
                        }
                    }
                }
                Inst::Look { negate, end } => {
                    let mut sub = regs.to_vec();
                    let r = self.run(pc as u32 + 1, pos, &mut sub);
                    match (r.is_some(), negate) {
                        (true, false) => {
                            for (k, v) in sub.iter().enumerate() {
                                if regs[k] != *v {
                                    stack.push(Bt::Restore { reg: k as u32, val: regs[k] });
                                    regs[k] = *v;
                                }
                            }
                            pc = end as usize;
                            true
                        }
                        (false, true) => {
                            pc = end as usize;
                            true
                        }
                        _ => false,
                    }
                }
                Inst::SetReg(r) => {
                    self.set(regs, stack, r, pos);
                    pc += 1;
                    true
                }
                Inst::EmptyCheck(r) => {
                    pc += 1;
                    regs[r as usize] != pos
                }
                Inst::EmptyCheckMin { pos: r, count, min } => {
                    pc += 1;
                    !(regs[r as usize] == pos && regs[count as usize] as u32 >= min)
                }
                Inst::CountInit(r) => {
                    self.set(regs, stack, r, 0);
                    pc += 1;
                    true
                }
                Inst::CountInc(r) => {
                    let v = regs[r as usize].saturating_add(1);
                    self.set(regs, stack, r, v);
                    pc += 1;
                    true
                }
                Inst::CountLt { r, n, target } => {
                    if (regs[r as usize] as u64) < n as u64 {
                        pc = target as usize;
                    } else {
                        pc += 1;
                    }
                    true
                }
                Inst::CountGe { r, n, target } => {
                    if regs[r as usize] as u64 >= n as u64 {
                        pc = target as usize;
                    } else {
                        pc += 1;
                    }
                    true
                }
                Inst::Succeed | Inst::Match => return Some(pos),
            };
            if ok {
                continue;
            }
            // retrocede
            loop {
                match stack.pop() {
                    None => return None,
                    Some(Bt::Restore { reg, val }) => regs[reg as usize] = val,
                    Some(Bt::Branch { pc: p, pos: q }) => {
                        pc = p as usize;
                        pos = q;
                        break;
                    }
                    Some(Bt::Greedy { pc: p, min_pos, cur, back }) => {
                        // devolve um caractere
                        let prev = if back {
                            self.read_fwd(cur).map(|(_, n)| n)
                        } else {
                            self.read_back(cur).map(|(_, n)| n)
                        };
                        let Some(prev) = prev else { continue };
                        if prev != min_pos {
                            stack.push(Bt::Greedy { pc: p, min_pos, cur: prev, back });
                        }
                        pc = p as usize;
                        pos = prev;
                        break;
                    }
                    Some(Bt::Lazy { pc: p, rep, cur, count }) => {
                        let Inst::Repeat { m, max, back, .. } = insts[rep as usize] else { continue };
                        let Some(np) = self.step(m, cur, back) else { continue };
                        if count + 1 < max {
                            stack.push(Bt::Lazy { pc: p, rep, cur: np, count: count + 1 });
                        }
                        pc = p as usize;
                        pos = np;
                        break;
                    }
                }
            }
        }
    }
}

impl Program {
    /// Tenta casar começando exatamente em `pos`. Devolve as capturas
    /// (pares início/fim; NONE = grupo não participou).
    fn matcher<'a>(&'a self, s: &'a [u16]) -> Matcher<'a> {
        Matcher {
            prog: self,
            s,
            unicode: self.flags.full_unicode(),
            icase: self.flags.ignore_case,
            steps: 0,
            pool: Vec::new(),
        }
    }

    pub fn match_at(&self, s: &[u16], pos: usize) -> Option<Vec<usize>> {
        let mut m = self.matcher(s);
        let mut regs = vec![NONE; self.nregs as usize];
        m.run(0, pos, &mut regs)?;
        regs.truncate(2 * (self.ngroups as usize + 1));
        Some(regs)
    }

    /// Procura a partir de `from` (ou só em `from`, se `sticky`).
    pub fn search(&self, s: &[u16], from: usize, sticky: bool) -> Option<Vec<usize>> {
        if sticky {
            return if from <= s.len() { self.match_at(s, from) } else { None };
        }
        if self.anchored {
            return if from == 0 { self.match_at(s, 0) } else { None };
        }
        let unicode = self.flags.full_unicode();
        let mut m = self.matcher(s);
        let mut regs = vec![NONE; self.nregs as usize];
        let mut pos = from;
        while pos <= s.len() {
            if let Some(f) = &self.first {
                if pos >= s.len() {
                    return None;
                }
                match f.find(s, pos) {
                    Some(k) => pos = k,
                    None => return None,
                }
            }
            regs.fill(NONE);
            if m.run(0, pos, &mut regs).is_some() {
                regs.truncate(2 * (self.ngroups as usize + 1));
                return Some(regs);
            }
            pos += if unicode
                && pos + 1 < s.len()
                && (0xD800..0xDC00).contains(&s[pos])
                && (0xDC00..0xE000).contains(&s[pos + 1])
            {
                2
            } else {
                1
            };
        }
        None
    }
}

