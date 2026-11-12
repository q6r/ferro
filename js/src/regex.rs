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
