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
//!
//! Classes, `\d`, `\p{...}` e afins viram listas de faixas já na análise (as
//! propriedades vêm das tabelas do Unicode 17 em `unicode_props.rs`). Sem
//! diferenciar maiúsculas, a classe já sai fechada pelas equivalências de
//! caixa e com a negação aplicada: casar é uma busca binária.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use crate::unicode_props as up;

pub const NONE: usize = usize::MAX;
const INF: u32 = u32::MAX;
const MAXCP: u32 = 0x10FFFF;

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

// ------------------------------------------------------ conjuntos de faixas

/// Faixas inclusivas de pontos de código, ordenadas e sem sobreposição.
type Ranges = Vec<(u32, u32)>;

/// Ordena e junta faixas sobrepostas ou vizinhas.
fn norm(mut v: Ranges) -> Ranges {
    v.sort_unstable();
    let mut out: Ranges = Vec::with_capacity(v.len());
    for (a, b) in v {
        if let Some(last) = out.last_mut()
            && a <= last.1.saturating_add(1)
        {
            last.1 = last.1.max(b);
            continue;
        }
        out.push((a, b));
    }
    out
}

fn union(a: &[(u32, u32)], b: &[(u32, u32)]) -> Ranges {
    if b.is_empty() {
        return a.to_vec();
    }
    let mut v = Vec::with_capacity(a.len() + b.len());
    v.extend_from_slice(a);
    v.extend_from_slice(b);
    norm(v)
}

/// Complemento em relação a todos os pontos de código.
fn complement(a: &[(u32, u32)]) -> Ranges {
    let mut out = Vec::with_capacity(a.len() + 1);
    let mut prev = 0u32;
    for &(x, y) in a {
        if x > prev {
            out.push((prev, x - 1));
        }
        prev = y + 1;
    }
    if prev <= MAXCP {
        out.push((prev, MAXCP));
    }
    out
}

fn intersect(a: &[(u32, u32)], b: &[(u32, u32)]) -> Ranges {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if lo <= hi {
            out.push((lo, hi));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

fn subtract(a: &[(u32, u32)], b: &[(u32, u32)]) -> Ranges {
    if b.is_empty() {
        return a.to_vec();
    }
    intersect(a, &complement(b))
}

fn has(a: &[(u32, u32)], c: u32) -> bool {
    let i = a.partition_point(|&(_, y)| y < c);
    i < a.len() && a[i].0 <= c
}

// -------------------------------------------------------------------- caixa

/// Valor de uma tabela (código, valor) ordenada pelo código.
fn table_get(t: &[(u32, u32)], c: u32) -> Option<u32> {
    t.binary_search_by_key(&c, |&(k, _)| k).ok().map(|i| t[i].1)
}

/// Canonicalize do spec: com u/v, a dobra simples (CaseFolding.txt, C e S);
/// sem elas, a maiúscula quando é um caractere só (e não cai no ASCII).
pub fn canon(c: u32, unicode: bool) -> u32 {
    if c < 0x80 {
        return if unicode {
            if (0x41..=0x5A).contains(&c) { c + 32 } else { c }
        } else if (0x61..=0x7A).contains(&c) {
            c - 32
        } else {
            c
        };
    }
    table_get(if unicode { &up::SCF } else { &up::UPPER }, c).unwrap_or(c)
}

/// Tabela do Canonicalize e a inversa (canônico → códigos que levam a ele).
fn case_tables(unicode: bool) -> (&'static [(u32, u32)], &'static [(u32, u32)]) {
    if unicode { (&up::SCF, &up::SCF_INV) } else { (&up::UPPER, &up::UPPER_INV) }
}

/// Põe em `out` os caracteres com o mesmo canônico de `c` (ele incluso).
fn push_case_class(c: u32, unicode: bool, out: &mut Ranges) {
    let k = canon(c, unicode);
    out.push((k, k));
    let inv = case_tables(unicode).1;
    let i = inv.partition_point(|&(d, _)| d < k);
    for &(d, s) in &inv[i..] {
        if d != k {
            break;
        }
        out.push((s, s));
    }
}

thread_local! {
    /// Fechamentos de conjuntos grandes já calculados (o mesmo `\p{L}` com i
    /// em várias regex não refaz a conta).
    static FECHADOS: RefCell<HashMap<(Ranges, bool), Rc<Ranges>>> = RefCell::new(HashMap::new());
}

/// Fecha o conjunto pelas equivalências de caixa: sem diferenciar
/// maiúsculas, `c` casa se algum `a` do conjunto tem o mesmo canônico.
fn close_case(a: &[(u32, u32)], unicode: bool) -> Ranges {
    if a.is_empty() || a.first() == Some(&(0, MAXCP)) {
        return a.to_vec();
    }
    let grande = a.len() >= 32;
    if grande && let Some(r) = FECHADOS.with(|c| c.borrow().get(&(a.to_vec(), unicode)).cloned()) {
        return (*r).clone();
    }
    let (map, inv) = case_tables(unicode);
    // canônicos tocados: de quem muda ao canonicalizar e de quem já é canônico
    let mut ds: Vec<u32> = Vec::new();
    for &(lo, hi) in a {
        let i = map.partition_point(|&(s, _)| s < lo);
        ds.extend(map[i..].iter().take_while(|&&(s, _)| s <= hi).map(|&(_, d)| d));
        let j = inv.partition_point(|&(d, _)| d < lo);
        ds.extend(inv[j..].iter().take_while(|&&(d, _)| d <= hi).map(|&(d, _)| d));
    }
    ds.sort_unstable();
    ds.dedup();
    // cada classe tocada entra inteira (só o que ainda falta)
    let mut extra = Vec::new();
    let mut j = 0;
    for d in ds {
        if !has(a, d) {
            extra.push((d, d));
        }
        j += inv[j..].partition_point(|&(x, _)| x < d);
        for &(x, s) in &inv[j..] {
            if x != d {
                break;
            }
            if !has(a, s) {
                extra.push((s, s));
            }
        }
    }
    let r = if extra.is_empty() { a.to_vec() } else { union(a, &extra) };
    if grande {
        FECHADOS.with(|c| {
            let mut c = c.borrow_mut();
            if c.len() >= 64 {
                c.clear();
            }
            c.insert((a.to_vec(), unicode), Rc::new(r.clone()));
        });
    }
    r
}

/// MaybeSimpleCaseFolding (flag v com i): cada caractere vira a sua dobra.
fn fold_ranges(a: &[(u32, u32)]) -> Ranges {
    let mut from = Vec::new();
    let mut to = Vec::new();
    for &(lo, hi) in a {
        let i = up::SCF.partition_point(|&(s, _)| s < lo);
        for &(s, d) in up::SCF[i..].iter().take_while(|&&(s, _)| s <= hi) {
            from.push((s, s));
            to.push((d, d));
        }
    }
    if from.is_empty() {
        return a.to_vec();
    }
    union(&subtract(a, &norm(from)), &norm(to))
}

/// AllCharacters da flag v com i: os que a dobra simples não muda.
fn folded_chars() -> Ranges {
    complement(&norm(up::SCF.iter().map(|&(s, _)| (s, s)).collect()))
}

// ---------------------------------------------------- propriedades Unicode

/// Procura numa tabela (nome, valor) ordenada pelo nome (nome exato).
fn lookup<T: Copy>(t: &[(&str, T)], name: &str) -> Option<T> {
    t.binary_search_by(|(n, _)| (*n).cmp(name)).ok().map(|i| t[i].1)
}

/// Faixas das categorias gerais da máscara (bits de `GC_LEAF`).
fn gc_ranges(mask: u32) -> Ranges {
    let (st, va) = (&up::GC_START, &up::GC_VALUE);
    let mut out: Ranges = Vec::new();
    for k in 0..st.len() {
        if mask & (1 << va[k]) == 0 {
            continue;
        }
        let (a, b) = (st[k], st.get(k + 1).map_or(MAXCP, |&n| n - 1));
        match out.last_mut() {
            Some(last) if last.1 + 1 == a => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    out
}

/// Faixas de uma escrita (Script; com `ext`, Script_Extensions).
fn script_ranges(sc: u8, ext: bool) -> Ranges {
    let (st, va) = (&up::SC_START, &up::SC_VALUE);
    let mut out: Ranges = Vec::new();
    for k in 0..st.len() {
        if va[k] != sc {
            continue;
        }
        let (a, b) = (st[k], st.get(k + 1).map_or(MAXCP, |&n| n - 1));
        match out.last_mut() {
            Some(last) if last.1 + 1 == a => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    if ext {
        let over: Ranges = up::SCX_RANGES.iter().map(|&(a, b, _)| (a, b)).collect();
        let add: Ranges = up::SCX_RANGES
            .iter()
            .filter(|&&(_, _, k)| up::SCX_SETS[k as usize].contains(&sc))
            .map(|&(a, b, _)| (a, b))
            .collect();
        out = union(&subtract(&out, &over), &add);
    }
    out
}

/// O que um `\p{...}` designa.
enum Prop {
    /// Propriedade de caracteres (faixas vazias se só estamos validando).
    Chars(Ranges),
    /// Propriedade de strings (índice em `STRING_PROPS`, só com a flag v).
    Strings(usize),
}

/// Procura a propriedade pelos nomes exatos do spec (sem casamento frouxo).
/// Com `build` falso só confere o nome.
fn find_property(name: &str, value: Option<&str>, build: bool) -> Option<Prop> {
    let chars = |f: &dyn Fn() -> Ranges| Some(Prop::Chars(if build { f() } else { Vec::new() }));
    if let Some(v) = value {
        return match name {
            "General_Category" | "gc" => {
                let m = lookup(&up::GC_ALIASES, v)?;
                chars(&|| gc_ranges(m))
            }
            "Script" | "sc" | "Script_Extensions" | "scx" => {
                let s = lookup(&up::SC_ALIASES, v)?;
                let ext = name.starts_with("Script_") || name == "scx";
                chars(&|| script_ranges(s, ext))
            }
            _ => None,
        };
    }
    if let Some(m) = lookup(&up::GC_ALIASES, name) {
        return chars(&|| gc_ranges(m));
    }
    if let Some(r) = lookup(&up::BINARY, name) {
        return match name {
            "Any" => chars(&|| vec![(0, MAXCP)]),
            "ASCII" => chars(&|| vec![(0, 0x7F)]),
            "Assigned" => chars(&|| complement(&gc_ranges(lookup(&up::GC_ALIASES, "Cn").unwrap_or(0)))),
            _ => chars(&|| r.to_vec()),
        };
    }
    let k = up::STRING_PROPS.iter().position(|(n, _, _)| *n == name)?;
    Some(Prop::Strings(k))
}

/// Caracteres avulsos e strings de uma propriedade de strings.
fn string_prop(k: usize) -> (Ranges, Vec<Vec<u32>>) {
    let (_, (a, b), singles) = up::STRING_PROPS[k];
    let mut strs = Vec::new();
    let mut i = a;
    while i < b {
        let n = up::EMOJI_SEQS[i] as usize;
        strs.push(up::EMOJI_SEQS[i + 1..i + 1 + n].to_vec());
        i += 1 + n;
    }
    (singles.to_vec(), strs)
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

/// WhiteSpace ou LineTerminator do JS (o `\s`).
pub fn is_space(c: u32) -> bool {
    has(SPACE_RANGES, c)
}
const WORD_RANGES: &[(u32, u32)] = &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)];

// ----------------------------------------------------------------- classes

/// Classe pronta para casar: faixas (já com caixa e negação) e o ASCII num
/// mapa de bits.
#[derive(Clone, Debug, Default)]
pub struct ClassSet {
    ranges: Ranges,
    ascii: u128,
}

impl ClassSet {
    fn new(ranges: Ranges) -> ClassSet {
        let mut ascii = 0u128;
        for &(a, b) in &ranges {
            if a >= 128 {
                break;
            }
            for c in a..=b.min(127) {
                ascii |= 1 << c;
            }
        }
        ClassSet { ranges, ascii }
    }

    #[inline]
    fn contains(&self, c: u32) -> bool {
        if c < 128 {
            return (self.ascii >> c) & 1 != 0;
        }
        has(&self.ranges, c)
    }
}

/// Classe com strings (flag v): tenta as strings da mais longa para a mais
/// curta, depois os caracteres avulsos e por fim a string vazia.
#[derive(Clone, Debug)]
struct StrSet {
    /// Strings de 2+ pontos de código, pelo primeiro ponto e, nele, da mais
    /// longa para a mais curta.
    strs: Vec<Vec<u32>>,
    /// Índices de `strs` da mais longa para a mais curta (casar para trás).
    by_len: Vec<u32>,
    singles: ClassSet,
    empty: bool,
    /// Sem diferenciar maiúsculas (as strings já estão dobradas).
    icase: bool,
}

/// Conjunto da flag v durante a análise: caracteres e strings (tamanho ≠ 1).
#[derive(Clone, Debug, Default)]
struct VSet {
    ranges: Ranges,
    strs: BTreeSet<Vec<u32>>,
}

impl VSet {
    fn chars(ranges: Ranges) -> VSet {
        VSet { ranges, strs: BTreeSet::new() }
    }

    fn union(mut self, o: VSet) -> VSet {
        self.ranges = union(&self.ranges, &o.ranges);
        self.strs.extend(o.strs);
        self
    }

    fn intersect(mut self, o: VSet) -> VSet {
        self.ranges = intersect(&self.ranges, &o.ranges);
        self.strs.retain(|s| o.strs.contains(s));
        self
    }

    fn subtract(mut self, o: VSet) -> VSet {
        self.ranges = subtract(&self.ranges, &o.ranges);
        self.strs.retain(|s| !o.strs.contains(s));
        self
    }
}

// ------------------------------------------------------------------- árvore

#[derive(Clone, Debug)]
enum Node {
    Empty,
    /// Caractere exato.
    Char(u32),
    Class(ClassSet),
    /// Classe com strings (flag v).
    Strings(Box<StrSet>),
    /// `.` (true = dotAll).
    Any(bool),
    /// `^` e `$` (true = multiline).
    Start(bool),
    End(bool),
    /// `\b`/`\B`; `uicase`: u/v com i, em que ſ e K contam como letras.
    WordB { neg: bool, uicase: bool },
    Group(Box<Node>, Option<u32>),
    Look { ahead: bool, negate: bool, node: Box<Node> },
    BackRef { group: u32, icase: bool },
    NamedRef { name: String, icase: bool },
    Repeat { node: Box<Node>, min: u32, max: u32, greedy: bool },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
}

/// Modificadores em vigor (flags, ou `(?ims-ims:...)` em volta).
#[derive(Clone, Copy)]
struct Mods {
    icase: bool,
    multiline: bool,
    dot_all: bool,
}

struct Parser {
    /// Pontos de código (u/v) ou unidades UTF-16 (sem elas).
    s: Vec<u32>,
    i: usize,
    /// Flag u ou v.
    u: bool,
    /// Flag v.
    v: bool,
    /// Só valida a sintaxe (o lexer): não monta os conjuntos.
    check: bool,
    mods: Mods,
    /// Há grupos nomeados (então `\k` é referência mesmo sem a flag u).
    has_named: bool,
    total_groups: u32,
    ngroups: u32,
    /// Grupos nomeados: (nome, índice, alternativas em volta).
    named: Vec<(String, u32, Vec<(u32, u32)>)>,
    /// Alternativa atual de cada disjunção aberta: (disjunção, alternativa).
    path: Vec<(u32, u32)>,
    ndisj: u32,
    /// Nomes usados em `\k<...>` (conferidos no fim).
    refs: Vec<String>,
}

fn err<T>(msg: &str) -> Result<T, RegexError> {
    Err(RegexError(msg.into()))
}

fn is_syntax(c: u32) -> bool {
    matches!(char::from_u32(c), Some('^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'))
}

/// Pontuação que não pode aparecer dobrada numa classe da flag v (`&&`, `!!`...).
fn is_double_punct(c: u32) -> bool {
    matches!(char::from_u32(c), Some('&' | '!' | '#' | '$' | '%' | '*' | '+' | ',' | '.' | ':' | ';' | '<' | '=' | '>' | '?' | '@' | '^' | '`' | '~'))
}

/// Dois grupos estão em alternativas diferentes de alguma disjunção (nunca
/// participam juntos do casamento)?
fn exclusive(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    for (x, y) in a.iter().zip(b) {
        if x.0 != y.0 {
            return false;
        }
        if x.1 != y.1 {
            return true;
        }
    }
    false
}

fn is_id_start(c: u32) -> bool {
    c == '$' as u32 || c == '_' as u32 || has(&up::BP_ID_START, c)
}

fn is_id_continue(c: u32) -> bool {
    c == '$' as u32 || c == 0x200C || c == 0x200D || has(&up::BP_ID_CONTINUE, c)
}

impl Parser {
    fn new(pattern: &[u16], flags: Flags, check: bool) -> Parser {
        let u = flags.full_unicode();
        // com u/v, o padrão é lido por pontos de código (pares viram um só)
        let mut s = Vec::with_capacity(pattern.len());
        let mut i = 0;
        while i < pattern.len() {
            let c = pattern[i] as u32;
            if u && (0xD800..0xDC00).contains(&c) && i + 1 < pattern.len() && (0xDC00..0xE000).contains(&(pattern[i + 1] as u32)) {
                s.push(0x10000 + ((c - 0xD800) << 10) + (pattern[i + 1] as u32 - 0xDC00));
                i += 2;
            } else {
                s.push(c);
                i += 1;
            }
        }
        Parser {
            s,
            i: 0,
            u,
            v: flags.unicode_sets,
            check,
            mods: Mods {
                icase: flags.ignore_case,
                multiline: flags.multiline,
                dot_all: flags.dot_all,
            },
            has_named: false,
            total_groups: 0,
            ngroups: 0,
            named: Vec::new(),
            path: Vec::new(),
            ndisj: 0,
            refs: Vec::new(),
        }
    }

    /// Analisa o padrão inteiro.
    fn parse(&mut self) -> Result<Node, RegexError> {
        self.prescan();
        let tree = self.disjunction()?;
        if self.i < self.s.len() {
            return err(if self.s[self.i] == ')' as u32 { "Unmatched ')'" } else { "Unexpected character" });
        }
        for r in &self.refs {
            if !self.named.iter().any(|(n, _, _)| n == r) {
                return err("Invalid named capture referenced");
            }
        }
        Ok(tree)
    }

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

    fn at2(&self, a: char, b: char) -> bool {
        self.peek() == Some(a as u32) && self.peek_at(1) == Some(b as u32)
    }

    /// Conta grupos de captura e vê se há nomes (antes de analisar, para `\5` e `\k`).
    fn prescan(&mut self) {
        let mut i = 0;
        let mut depth = 0u32;
        while i < self.s.len() {
            let c = self.s[i];
            if c == '\\' as u32 {
                i += 2;
                continue;
            }
            if depth > 0 {
                if c == ']' as u32 {
                    depth -= 1;
                } else if c == '[' as u32 && self.v {
                    depth += 1;
                }
            } else if c == '[' as u32 {
                depth = 1;
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
        let id = self.ndisj;
        self.ndisj += 1;
        self.path.push((id, 0));
        let mut alts = vec![self.alternative()?];
        while self.eat('|') {
            if let Some(p) = self.path.last_mut() {
                p.1 += 1;
            }
            alts.push(self.alternative()?);
        }
        self.path.pop();
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
                return Ok(Node::Start(self.mods.multiline));
            }
            Some('$') => {
                self.i += 1;
                return Ok(Node::End(self.mods.multiline));
            }
            Some('\\') if matches!(self.peek_at(1).and_then(char::from_u32), Some('b' | 'B')) => {
                let neg = self.peek_at(1) == Some('B' as u32);
                self.i += 2;
                return Ok(Node::WordB {
                    neg,
                    uicase: self.u && self.mods.icase,
                });
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
                    if self.at_quantifier() {
                        // Anexo B: lookahead pode ter quantificador sem a flag u
                        if ahead && !self.u {
                            return self.quantifier(n);
                        }
                        return err("Invalid quantifier");
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
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            greedy,
        })
    }

    /// Nome de grupo (depois do '<', até o '>'): RegExpIdentifierName.
    fn group_name(&mut self) -> Result<String, RegexError> {
        let bad = || RegexError("Invalid capture group name".into());
        let mut name = String::new();
        loop {
            let c = self.peek().ok_or_else(bad)?;
            self.i += 1;
            if c == '>' as u32 {
                break;
            }
            let mut ch = c;
            if c == '\\' as u32 {
                if !self.eat('u') {
                    return Err(bad());
                }
                ch = self.name_escape().ok_or_else(bad)?;
            } else if (0xD800..0xDC00).contains(&c)
                && let Some(lo) = self.peek().filter(|l| (0xDC00..0xE000).contains(l))
            {
                // par substituto literal (sem a flag u o padrão está em unidades)
                self.i += 1;
                ch = 0x10000 + ((c - 0xD800) << 10) + (lo - 0xDC00);
            }
            let ok = if name.is_empty() { is_id_start(ch) } else { is_id_continue(ch) };
            let Some(ch) = char::from_u32(ch).filter(|_| ok) else { return Err(bad()) };
            name.push(ch);
        }
        if name.is_empty() {
            return Err(bad());
        }
        Ok(name)
    }

    /// Escape de nome de grupo, depois de `\u`: `{...}`, `XXXX` ou um par de `\uXXXX`.
    fn name_escape(&mut self) -> Option<u32> {
        if self.eat('{') {
            let start = self.i;
            let mut v: u32 = 0;
            while let Some(d) = self.peek().and_then(char::from_u32).and_then(|c| c.to_digit(16)) {
                v = v.saturating_mul(16).saturating_add(d);
                self.i += 1;
            }
            if self.i == start || !self.eat('}') || v > MAXCP {
                return None;
            }
            return Some(v);
        }
        let v = self.hex(4)?;
        if (0xD800..0xDC00).contains(&v) && self.peek() == Some('\\' as u32) && self.peek_at(1) == Some('u' as u32) {
            let save = self.i;
            self.i += 2;
            match self.hex(4) {
                Some(lo) if (0xDC00..0xE000).contains(&lo) => return Some(0x10000 + ((v - 0xD800) << 10) + (lo - 0xDC00)),
                _ => self.i = save,
            }
        }
        Some(v)
    }

    /// Registra um grupo nomeado; o mesmo nome só pode repetir em alternativas diferentes.
    fn add_named(&mut self, name: String, idx: u32) -> Result<(), RegexError> {
        if self.named.iter().any(|(n, _, path)| *n == name && !exclusive(path, &self.path)) {
            return err("Duplicate capture group name");
        }
        self.named.push((name, idx, self.path.clone()));
        Ok(())
    }

    fn atom(&mut self) -> Result<Node, RegexError> {
        let c = self.peek().ok_or(RegexError("Unexpected end".into()))?;
        match char::from_u32(c) {
            Some('.') => {
                self.i += 1;
                Ok(Node::Any(self.mods.dot_all))
            }
            Some('(') => {
                self.i += 1;
                if self.eat('?') {
                    if self.eat(':') {
                        return self.group_body(None);
                    }
                    if self.eat('<') {
                        let name = self.group_name()?;
                        self.ngroups += 1;
                        let idx = self.ngroups;
                        self.add_named(name, idx)?;
                        return self.group_body(Some(idx));
                    }
                    return self.modifiers_group();
                }
                self.ngroups += 1;
                let idx = self.ngroups;
                self.group_body(Some(idx))
            }
            Some(')') => err("Unmatched ')'"),
            Some('[') => {
                self.i += 1;
                if self.v {
                    let set = self.class_v_top()?;
                    return Ok(self.class_node(set));
                }
                let (ranges, negated) = self.class_classic()?;
                if self.check {
                    return Ok(Node::Class(ClassSet::default()));
                }
                let mut r = if self.mods.icase { close_case(&ranges, self.u) } else { ranges };
                if negated {
                    r = complement(&r);
                }
                Ok(Node::Class(ClassSet::new(r)))
            }
            Some('*' | '+' | '?') => err("Nothing to repeat"),
            Some('{') => {
                if self.u || self.brace_quantifier().is_some() {
                    return err(if self.u { "Lone quantifier brackets" } else { "Nothing to repeat" });
                }
                self.i += 1;
                Ok(self.char_node(c))
            }
            Some('}' | ']') if self.u => err("Lone quantifier brackets"),
            Some('\\') => {
                self.i += 1;
                self.atom_escape()
            }
            _ => {
                self.i += 1;
                Ok(self.char_node(c))
            }
        }
    }

    /// Corpo de um grupo até o ')'.
    fn group_body(&mut self, idx: Option<u32>) -> Result<Node, RegexError> {
        let d = self.disjunction()?;
        if !self.eat(')') {
            return err("Unterminated group");
        }
        Ok(Node::Group(Box::new(d), idx))
    }

    /// `(?ims-ims:...)` (depois do "(?"): muda i, m e s só dentro do grupo.
    fn modifiers_group(&mut self) -> Result<Node, RegexError> {
        let mut add = Vec::new();
        let mut remove = Vec::new();
        let mut dash = false;
        loop {
            match self.peek().and_then(char::from_u32) {
                Some(f @ ('i' | 'm' | 's')) => {
                    if add.contains(&f) || remove.contains(&f) {
                        return err("Repeated flag in flag group");
                    }
                    if dash { remove.push(f) } else { add.push(f) }
                    self.i += 1;
                }
                Some('-') if dash => return err("Multiple dashes in flag group"),
                Some('-') => {
                    dash = true;
                    self.i += 1;
                }
                Some(':') if !add.is_empty() || !remove.is_empty() => {
                    self.i += 1;
                    break;
                }
                Some(':') => return err("Invalid flag group"),
                _ => return err("Invalid group"),
            }
        }
        let saved = self.mods;
        for (list, on) in [(&add, true), (&remove, false)] {
            for f in list.iter() {
                match f {
                    'i' => self.mods.icase = on,
                    'm' => self.mods.multiline = on,
                    _ => self.mods.dot_all = on,
                }
            }
        }
        let r = self.group_body(None);
        self.mods = saved;
        r
    }

    /// Um caractere literal: sem diferenciar maiúsculas, vira a classe dos equivalentes.
    fn char_node(&self, c: u32) -> Node {
        if self.mods.icase && !self.check {
            let mut eq = Vec::new();
            push_case_class(c, self.u, &mut eq);
            if eq.len() > 1 {
                return Node::Class(ClassSet::new(norm(eq)));
            }
        }
        Node::Char(c)
    }

    /// Classe pronta a partir de um conjunto (fecha pela caixa com i).
    fn class_node(&self, set: VSet) -> Node {
        if self.check {
            return Node::Class(ClassSet::default());
        }
        let ranges = if self.mods.icase { close_case(&set.ranges, self.u) } else { set.ranges };
        if set.strs.is_empty() {
            return Node::Class(ClassSet::new(ranges));
        }
        let mut strs: Vec<Vec<u32>> = set.strs.into_iter().collect();
        let empty = strs.first().is_some_and(|s| s.is_empty());
        if empty {
            strs.remove(0);
        }
        strs.sort_by(|a, b| a[0].cmp(&b[0]).then(b.len().cmp(&a.len())));
        let mut by_len: Vec<u32> = (0..strs.len() as u32).collect();
        by_len.sort_by_key(|&k| std::cmp::Reverse(strs[k as usize].len()));
        Node::Strings(Box::new(StrSet {
            strs,
            by_len,
            singles: ClassSet::new(ranges),
            empty,
            icase: self.mods.icase,
        }))
    }

    /// CharacterComplement: com v e i, só entre os caracteres já dobrados.
    fn char_complement(&self, a: &[(u32, u32)]) -> Ranges {
        if self.check {
            Vec::new()
        } else if self.v && self.mods.icase {
            subtract(&folded_chars(), a)
        } else {
            complement(a)
        }
    }

    /// MaybeSimpleCaseFolding das faixas (só com v e i).
    fn fold(&self, r: Ranges) -> Ranges {
        if self.v && self.mods.icase && !self.check { fold_ranges(&r) } else { r }
    }

    /// `\d \D \s \S \w \W` como o spec monta o conjunto.
    fn class_escape(&self, c: char) -> Option<Ranges> {
        if !matches!(c, 'd' | 'D' | 's' | 'S' | 'w' | 'W') {
            return None;
        }
        if self.check {
            return Some(Vec::new());
        }
        let word = || {
            let mut w = WORD_RANGES.to_vec();
            if self.u && self.mods.icase {
                w.extend([(0x17F, 0x17F), (0x212A, 0x212A)]);
            }
            norm(w)
        };
        Some(match c {
            'd' => DIGIT_RANGES.to_vec(),
            'D' => self.char_complement(DIGIT_RANGES),
            's' => SPACE_RANGES.to_vec(),
            'S' => self.char_complement(SPACE_RANGES),
            'w' => self.fold(word()),
            _ => self.char_complement(&word()),
        })
    }

    /// `\p{...}`/`\P{...}` (o 'p' já foi lido): (conjunto, pode ter strings).
    fn property(&mut self, negate: bool, in_class: bool) -> Result<(VSet, bool), RegexError> {
        let msg = if in_class { "Invalid property name in character class" } else { "Invalid property name" };
        if !self.eat('{') {
            return err(msg);
        }
        let mut text = String::new();
        loop {
            let Some(c) = self.peek() else { return err(msg) };
            self.i += 1;
            match char::from_u32(c) {
                Some('}') => break,
                Some(ch) if ch.is_ascii_alphanumeric() || ch == '_' || ch == '=' => text.push(ch),
                _ => return err(msg),
            }
        }
        let (name, value) = match text.split_once('=') {
            Some((a, b)) => (a, Some(b)),
            None => (text.as_str(), None),
        };
        match find_property(name, value, !self.check) {
            Some(Prop::Chars(r)) => {
                let r = self.fold(r);
                Ok((VSet::chars(if negate { self.char_complement(&r) } else { r }), false))
            }
            Some(Prop::Strings(k)) if self.v && !negate => {
                if self.check {
                    return Ok((VSet::default(), true));
                }
                let (singles, strs) = string_prop(k);
                let strs = strs.into_iter().map(|s| if self.mods.icase { s.into_iter().map(|c| canon(c, true)).collect() } else { s });
                Ok((
                    VSet {
                        ranges: self.fold(singles),
                        strs: strs.collect(),
                    },
                    true,
                ))
            }
            _ => err(msg),
        }
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
    fn unicode_escape_value(&mut self) -> Result<Option<u32>, RegexError> {
        if self.u && self.eat('{') {
            let start = self.i;
            let mut v: u32 = 0;
            while let Some(d) = self.peek().and_then(char::from_u32).and_then(|c| c.to_digit(16)) {
                v = v.saturating_mul(16).saturating_add(d);
                self.i += 1;
            }
            if self.i == start || !self.eat('}') || v > MAXCP {
                return err("Invalid Unicode escape");
            }
            return Ok(Some(v));
        }
        let Some(v) = self.hex(4) else { return Ok(None) };
        if self.u && (0xD800..0xDC00).contains(&v) && self.peek() == Some('\\' as u32) && self.peek_at(1) == Some('u' as u32) {
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
                            return err("Invalid Unicode escape");
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
            'u' => match self.unicode_escape_value()? {
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
        if let Some(set) = self.class_escape(ch) {
            return Ok(self.class_node(VSet::chars(set)));
        }
        if (ch == 'p' || ch == 'P') && self.u {
            let (set, _) = self.property(ch == 'P', false)?;
            return Ok(self.class_node(set));
        }
        if ch == 'k' && (self.u || self.has_named) {
            if !self.eat('<') {
                return err("Invalid named reference");
            }
            let name = self.group_name()?;
            self.refs.push(name.clone());
            return Ok(Node::NamedRef { name, icase: self.mods.icase });
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
                return Ok(Node::BackRef { group: n, icase: self.mods.icase });
            }
            if self.u {
                return err("Invalid escape");
            }
            self.i = start + 1;
            if ch >= '8' {
                return Ok(self.char_node(c));
            }
            let v = self.legacy_octal(c);
            return Ok(self.char_node(v));
        }
        if ch == '0' && self.peek().is_some_and(|d| (0x30..=0x39).contains(&d)) {
            if self.u {
                return err("Invalid decimal escape");
            }
            let v = self.legacy_octal(c);
            return Ok(self.char_node(v));
        }
        if let Some(v) = self.char_escape(ch, false)? {
            return Ok(self.char_node(v));
        }
        if self.u && !is_syntax(c) && ch != '/' {
            return err("Invalid escape");
        }
        Ok(self.char_node(c))
    }

    /// Classe sem a flag v (depois do '['): faixas (sem caixa) e se é negada.
    fn class_classic(&mut self) -> Result<(Ranges, bool), RegexError> {
        let negated = self.eat('^');
        let mut ranges = Vec::new();
        loop {
            let Some(c) = self.peek() else { return err("Unterminated character class") };
            if c == ']' as u32 {
                self.i += 1;
                break;
            }
            let a = self.class_atom(&mut ranges)?;
            if self.is('-') && self.peek_at(1).is_some_and(|x| x != ']' as u32) {
                self.i += 1;
                let mut tmp = Vec::new();
                let b = self.class_atom(&mut tmp)?;
                match (a, b) {
                    (Some(a), Some(b)) => {
                        if a > b {
                            return err("Range out of order in character class");
                        }
                        ranges.push((a, b));
                    }
                    _ => {
                        if self.u {
                            return err("Invalid character class");
                        }
                        // Anexo B: \d-x vira \d, '-', x
                        if let Some(a) = a {
                            ranges.push((a, a));
                        }
                        ranges.push(('-' as u32, '-' as u32));
                        if let Some(b) = b {
                            ranges.push((b, b));
                        }
                        ranges.extend(tmp);
                    }
                }
            } else if let Some(a) = a {
                ranges.push((a, a));
            }
        }
        Ok((norm(ranges), negated))
    }

    /// Um elemento de classe (sem a flag v): caractere (Some) ou conjunto já posto em `set` (None).
    fn class_atom(&mut self, set: &mut Ranges) -> Result<Option<u32>, RegexError> {
        let c = self.peek().ok_or(RegexError("Unterminated character class".into()))?;
        self.i += 1;
        if c != '\\' as u32 {
            return Ok(Some(c));
        }
        let Some(e) = self.peek() else { return err("\\ at end of pattern") };
        self.i += 1;
        let ch = char::from_u32(e).unwrap_or('\0');
        if let Some(s) = self.class_escape(ch) {
            set.extend(s);
            return Ok(None);
        }
        if (ch == 'p' || ch == 'P') && self.u {
            let (s, _) = self.property(ch == 'P', true)?;
            set.extend(s.ranges);
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
        if self.u && !is_syntax(e) && ch != '/' {
            return err("Invalid escape");
        }
        // Anexo B: com grupos nomeados, \k não é escape de identidade
        if ch == 'k' && self.has_named {
            return err("Invalid escape");
        }
        Ok(Some(e))
    }

    /// Classe da flag v (depois do '['), já com a negação.
    fn class_v_top(&mut self) -> Result<VSet, RegexError> {
        let neg = self.eat('^');
        let (set, may) = self.class_contents_v()?;
        if neg {
            if may {
                return err("Negated character class may contain strings");
            }
            return Ok(VSet::chars(self.char_complement(&set.ranges)));
        }
        Ok(set)
    }

    /// ClassContents da flag v até o ']' (consumido): (conjunto, MayContainStrings).
    fn class_contents_v(&mut self) -> Result<(VSet, bool), RegexError> {
        if self.eat(']') {
            return Ok((VSet::default(), false));
        }
        let (first, may0, range) = self.class_operand_v(true)?;
        let inter = self.at2('&', '&');
        if inter || self.at2('-', '-') {
            // interseção ou diferença: só operandos, todos com o mesmo operador
            if range {
                return err("Invalid set operation in character class");
            }
            let (mut acc, mut may) = (first, may0);
            loop {
                if self.eat(']') {
                    return Ok((acc, may));
                }
                if self.peek().is_none() {
                    return err("Unterminated character class");
                }
                if !(if inter { self.at2('&', '&') } else { self.at2('-', '-') }) {
                    return err("Invalid set operation in character class");
                }
                self.i += 2;
                if inter && self.is('&') {
                    return err("Invalid character in character class");
                }
                let (b, mb, _) = self.class_operand_v(false)?;
                if inter {
                    acc = acc.intersect(b);
                    may = may && mb;
                } else {
                    acc = acc.subtract(b);
                }
            }
        }
        let (mut acc, mut may) = (first, may0);
        loop {
            if self.eat(']') {
                return Ok((acc, may));
            }
            if self.at2('&', '&') || self.at2('-', '-') {
                return err("Invalid set operation in character class");
            }
            let (b, mb, _) = self.class_operand_v(true)?;
            acc = acc.union(b);
            may |= mb;
        }
    }

    /// Operando da flag v (ou faixa, se `range`): (conjunto, MayContainStrings, era faixa).
    fn class_operand_v(&mut self, range: bool) -> Result<(VSet, bool, bool), RegexError> {
        let Some(c) = self.peek() else { return err("Unterminated character class") };
        if c == '[' as u32 {
            self.i += 1;
            let neg = self.eat('^');
            let (set, may) = self.class_contents_v()?;
            if neg {
                if may {
                    return err("Negated character class may contain strings");
                }
                return Ok((VSet::chars(self.char_complement(&set.ranges)), false, false));
            }
            return Ok((set, may, false));
        }
        if c == '\\' as u32
            && let Some(e) = self.peek_at(1).and_then(char::from_u32)
        {
            if let Some(r) = self.class_escape(e) {
                self.i += 2;
                return Ok((VSet::chars(r), false, false));
            }
            if e == 'p' || e == 'P' {
                self.i += 2;
                let (set, may) = self.property(e == 'P', true)?;
                return Ok((set, may, false));
            }
            if e == 'q' {
                self.i += 2;
                if !self.eat('{') {
                    return err("Invalid escape");
                }
                let (set, may) = self.class_strings_v()?;
                return Ok((set, may, false));
            }
        }
        let a = self.class_set_char()?;
        if range && self.is('-') && self.peek_at(1) != Some('-' as u32) {
            self.i += 1;
            let b = self.class_set_char()?;
            if a > b {
                return err("Range out of order in character class");
            }
            return Ok((VSet::chars(self.fold(vec![(a, b)])), false, true));
        }
        Ok((VSet::chars(self.fold(vec![(a, a)])), false, false))
    }

    /// ClassSetCharacter: um caractere (com escapes) da flag v.
    fn class_set_char(&mut self) -> Result<u32, RegexError> {
        let Some(c) = self.peek() else { return err("Unterminated character class") };
        if c == '\\' as u32 {
            self.i += 1;
            let Some(e) = self.peek() else { return err("\\ at end of pattern") };
            self.i += 1;
            let ch = char::from_u32(e).unwrap_or('\0');
            if ch == 'b' {
                return Ok(0x08);
            }
            if "&-!#%,:;<=>@`~".contains(ch) || is_syntax(e) || ch == '/' {
                return Ok(e);
            }
            if let Some(v) = self.char_escape(ch, true)? {
                return Ok(v);
            }
            return err(if matches!(ch, 'd' | 'D' | 's' | 'S' | 'w' | 'W' | 'p' | 'P' | 'q') {
                "Invalid character class"
            } else {
                "Invalid escape"
            });
        }
        if self.peek_at(1) == Some(c) && is_double_punct(c) {
            return err("Invalid set operation in character class");
        }
        if matches!(char::from_u32(c), Some('(' | ')' | '[' | ']' | '{' | '}' | '/' | '-' | '\\' | '|')) {
            return err("Invalid character in character class");
        }
        self.i += 1;
        Ok(c)
    }

    /// `\q{...}` (depois do '{'): strings separadas por '|'.
    fn class_strings_v(&mut self) -> Result<(VSet, bool), RegexError> {
        let mut set = VSet::default();
        let mut may = false;
        let mut cur: Vec<u32> = Vec::new();
        loop {
            if self.is('}') || self.is('|') {
                let end = self.is('}');
                self.i += 1;
                let mut s = std::mem::take(&mut cur);
                if s.len() != 1 {
                    may = true;
                }
                if self.v && self.mods.icase {
                    s.iter_mut().for_each(|c| *c = canon(*c, true));
                }
                if s.len() == 1 {
                    set.ranges = union(&set.ranges, &[(s[0], s[0])]);
                } else {
                    set.strs.insert(s);
                }
                if end {
                    return Ok((set, may));
                }
                continue;
            }
            let c = self.class_set_char()?;
            cur.push(c);
        }
    }
}

// ----------------------------------------------------------------- bytecode

#[derive(Clone, Copy, Debug)]
enum Single {
    Char(u32),
    /// `.` com dotAll.
    Any,
    /// `.` sem dotAll (não casa quebras de linha).
    AnyNl,
    Class(u32),
}

#[derive(Clone, Copy, Debug)]
enum Inst {
    Single { m: Single, back: bool },
    Repeat { m: Single, min: u32, max: u32, greedy: bool, back: bool },
    /// Classe com strings (flag v): índice em `strsets`.
    Strings { set: u32, back: bool },
    Split(u32, u32),
    Jmp(u32),
    Save(u32),
    Clear(u32, u32),
    /// `^`/`$` (com multiline).
    Start(bool),
    End(bool),
    WordB { neg: bool, uicase: bool },
    BackRef { group: u32, back: bool, icase: bool },
    /// `\k<nome>` com o nome em vários grupos: casa o que participou (lista em `ref_lists`).
    BackRefAny { list: u32, back: bool, icase: bool },
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
    strsets: Vec<StrSet>,
    ref_lists: Vec<Vec<u32>>,
    /// Grupos de captura (sem contar o 0).
    pub ngroups: u32,
    nregs: u32,
    /// Grupos nomeados na ordem do padrão; um nome pode ter vários grupos
    /// (em alternativas diferentes).
    pub names: Vec<(String, Vec<u32>)>,
    pub flags: Flags,
    /// Unidades com que o casamento pode começar (acelera a busca).
    first: Option<FirstSet>,
    /// Começa com `^` sem multiline: só a posição 0 pode casar.
    anchored: bool,
}

/// Conjunto de unidades UTF-16 iniciais possíveis.
enum FirstSet {
    One(u16),
    Two(u16, u16),
    /// Mapa de bits das unidades < 256 e faixas das demais.
    Set { low: [u64; 4], high: Vec<(u16, u16)> },
}

impl FirstSet {
    fn find(&self, s: &[u16], from: usize) -> Option<usize> {
        let tail = &s[from..];
        let k = match self {
            FirstSet::One(c) => tail.iter().position(|x| x == c),
            FirstSet::Two(a, b) => tail.iter().position(|x| x == a || x == b),
            FirstSet::Set { low, high } => tail.iter().position(|&x| {
                if x < 256 {
                    low[(x >> 6) as usize] >> (x & 63) & 1 != 0
                } else {
                    let i = high.partition_point(|&(_, b)| b < x);
                    i < high.len() && high[i].0 <= x
                }
            }),
        };
        k.map(|k| from + k)
    }
}

struct Compiler {
    insts: Vec<Inst>,
    classes: Vec<ClassSet>,
    strsets: Vec<StrSet>,
    ref_lists: Vec<Vec<u32>>,
    nregs: u32,
    flags: Flags,
    names: HashMap<String, Vec<u32>>,
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
            Node::Any(true) => Single::Any,
            Node::Any(false) => Single::AnyNl,
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
            Node::Char(_) | Node::Any(_) | Node::Class(_) => {
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
            Node::Strings(ss) => {
                self.strsets.push((**ss).clone());
                let set = (self.strsets.len() - 1) as u32;
                self.emit(Inst::Strings { set, back });
            }
            Node::Start(m) => {
                self.emit(Inst::Start(*m));
            }
            Node::End(m) => {
                self.emit(Inst::End(*m));
            }
            Node::WordB { neg, uicase } => {
                self.emit(Inst::WordB { neg: *neg, uicase: *uicase });
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
            Node::BackRef { group, icase } => {
                self.emit(Inst::BackRef { group: *group, back, icase: *icase });
            }
            Node::NamedRef { name, icase } => {
                let gs = self.names.get(name).cloned().ok_or(RegexError("Invalid named capture referenced".into()))?;
                if gs.len() == 1 {
                    self.emit(Inst::BackRef { group: gs[0], back, icase: *icase });
                } else {
                    self.ref_lists.push(gs);
                    let list = (self.ref_lists.len() - 1) as u32;
                    self.emit(Inst::BackRefAny { list, back, icase: *icase });
                }
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
        let simple = matches!(node, Node::Any(_) | Node::Class(_)) || matches!(node, Node::Char(c) if *c <= 0xFFFF || self.flags.full_unicode());
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
            // iteração vazia não vale (o spec a descarta e segue sem ela)
            let p = nullable(node).then(|| self.reg());
            if let Some(p) = p {
                self.emit(Inst::SetReg(p));
            }
            clear(self);
            self.compile(node, back)?;
            if let Some(p) = p {
                self.emit(Inst::EmptyCheck(p));
            }
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

/// O nó pode casar a string vazia?
fn nullable(n: &Node) -> bool {
    match n {
        Node::Char(_) | Node::Class(_) | Node::Any(_) => false,
        Node::Strings(ss) => ss.empty,
        Node::Group(inner, _) => nullable(inner),
        Node::Repeat { node, min, .. } => *min == 0 || nullable(node),
        Node::Concat(items) => items.iter().all(nullable),
        Node::Alt(alts) => alts.iter().any(nullable),
        // vazio, asserções e referências
        _ => true,
    }
}

/// Unidade inicial de um ponto de código (o substituto alto, se astral).
fn first_unit(c: u32) -> u32 {
    if c > 0xFFFF { 0xD800 + ((c - 0x10000) >> 10) } else { c }
}

/// Unidades iniciais das faixas de pontos de código.
fn first_units(r: &[(u32, u32)], out: &mut Ranges) {
    for &(a, b) in r {
        if b > 0xFFFF {
            // astrais começam por um substituto alto
            out.push((first_unit(a.max(0x10000)), first_unit(b)));
            if a <= 0xFFFF {
                out.push((a, 0xFFFF));
            }
        } else {
            out.push((a, b));
        }
    }
}

/// Unidades com que o nó pode começar e se ele pode casar vazio
/// (None = qualquer unidade).
fn first_info(n: &Node) -> Option<(Ranges, bool)> {
    Some(match n {
        Node::Char(c) => (vec![(first_unit(*c), first_unit(*c))], false),
        Node::Class(cs) => {
            let mut v = Vec::new();
            first_units(&cs.ranges, &mut v);
            (v, false)
        }
        Node::Strings(ss) => {
            let mut cps = ss.singles.ranges.clone();
            cps.extend(ss.strs.iter().map(|s| (s[0], s[0])));
            if ss.icase {
                // a entrada é comparada pela dobra: vale qualquer equivalente
                cps = close_case(&norm(cps), true);
            }
            let mut v = Vec::new();
            first_units(&cps, &mut v);
            (v, ss.empty)
        }
        // asserções não consomem: o começo vem do que vier depois
        Node::Empty | Node::Start(_) | Node::End(_) | Node::WordB { .. } | Node::Look { .. } => (Vec::new(), true),
        Node::Any(_) | Node::BackRef { .. } | Node::NamedRef { .. } => return None,
        Node::Group(inner, _) => first_info(inner)?,
        Node::Repeat { node, min, .. } => {
            let (r, vazio) = first_info(node)?;
            (r, vazio || *min == 0)
        }
        Node::Concat(items) => {
            let mut v = Vec::new();
            for it in items {
                let (r, vazio) = first_info(it)?;
                v.extend(r);
                if !vazio {
                    return Some((v, false));
                }
            }
            (v, true)
        }
        Node::Alt(alts) => {
            let mut v = Vec::new();
            let mut vazio = false;
            for a in alts {
                let (r, z) = first_info(a)?;
                v.extend(r);
                vazio |= z;
            }
            (v, vazio)
        }
    })
}

fn first_set(n: &Node) -> Option<FirstSet> {
    let (r, vazio) = first_info(n)?;
    if vazio || r.is_empty() {
        return None;
    }
    let r = norm(r);
    let units: u32 = r.iter().map(|&(a, b)| b - a + 1).sum();
    if units == 1 {
        return Some(FirstSet::One(r[0].0 as u16));
    }
    if units == 2 {
        let a = r[0].0 as u16;
        let b = r.last().map_or(a, |x| x.1 as u16);
        return Some(FirstSet::Two(a, b));
    }
    if units >= 0xF000 {
        // quase tudo passa: o filtro só atrasaria
        return None;
    }
    let mut low = [0u64; 4];
    let mut high = Vec::new();
    for &(a, b) in &r {
        for c in a..=b.min(255) {
            low[(c >> 6) as usize] |= 1 << (c & 63);
        }
        if b >= 256 {
            high.push((a.max(256) as u16, b as u16));
        }
    }
    Some(FirstSet::Set { low, high })
}

fn starts_anchored(n: &Node) -> bool {
    match n {
        Node::Start(false) => true,
        Node::Concat(items) => items.first().is_some_and(starts_anchored),
        Node::Group(inner, _) => starts_anchored(inner),
        Node::Alt(alts) => alts.iter().all(starts_anchored),
        _ => false,
    }
}

/// Analisa e compila um padrão.
pub fn compile(pattern: &[u16], flags: Flags) -> Result<Program, RegexError> {
    let mut p = Parser::new(pattern, flags, false);
    let tree = p.parse()?;
    // nomes na ordem em que aparecem (um nome pode ter vários grupos)
    let mut names: Vec<(String, Vec<u32>)> = Vec::new();
    for (n, idx, _) in &p.named {
        match names.iter_mut().find(|(m, _)| m == n) {
            Some((_, v)) => v.push(*idx),
            None => names.push((n.clone(), vec![*idx])),
        }
    }
    let mut c = Compiler {
        insts: Vec::new(),
        classes: Vec::new(),
        strsets: Vec::new(),
        ref_lists: Vec::new(),
        nregs: 2 * (p.ngroups + 1),
        flags,
        names: names.iter().cloned().collect(),
    };
    c.emit(Inst::Save(0));
    c.compile(&tree, false)?;
    c.emit(Inst::Save(1));
    c.emit(Inst::Match);
    Ok(Program {
        insts: c.insts,
        classes: c.classes,
        strsets: c.strsets,
        ref_lists: c.ref_lists,
        ngroups: p.ngroups,
        nregs: c.nregs,
        names,
        flags,
        first: first_set(&tree),
        anchored: starts_anchored(&tree),
    })
}

/// Só confere a sintaxe e os erros antecipados (para os literais `/.../`).
pub fn validate(pattern: &[u16], flags: Flags) -> Result<(), RegexError> {
    Parser::new(pattern, flags, true).parse().map(|_| ())
}

// ----------------------------------------------------------------- execução

enum Bt {
    Branch { pc: u32, pos: usize },
    Restore { reg: u32, val: usize },
    Greedy { pc: u32, min_pos: usize, cur: usize, back: bool },
    Lazy { pc: u32, rep: u32, cur: usize, count: u32 },
}

/// `i` cai entre as duas metades de um par substituto?
fn mid_pair(s: &[u16], i: usize) -> bool {
    i > 0 && i < s.len() && (0xDC00..0xE000).contains(&s[i]) && (0xD800..0xDC00).contains(&s[i - 1])
}

fn is_line_term(c: u16) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

/// Lê um ponto de código de `s` em `pos` (pares juntos se `unicode`).
fn decode_fwd(s: &[u16], pos: usize, unicode: bool) -> Option<(u32, usize)> {
    let c = *s.get(pos)? as u32;
    if unicode
        && (0xD800..0xDC00).contains(&c)
        && let Some(&lo) = s.get(pos + 1)
        && (0xDC00..0xE000).contains(&(lo as u32))
    {
        return Some((0x10000 + ((c - 0xD800) << 10) + (lo as u32 - 0xDC00), pos + 2));
    }
    Some((c, pos + 1))
}

/// Lê o ponto de código que termina em `pos` (para trás).
fn decode_back(s: &[u16], pos: usize, unicode: bool) -> Option<(u32, usize)> {
    if pos == 0 {
        return None;
    }
    let c = s[pos - 1] as u32;
    if unicode && (0xDC00..0xE000).contains(&c) && pos >= 2 {
        let hi = s[pos - 2] as u32;
        if (0xD800..0xDC00).contains(&hi) {
            return Some((0x10000 + ((hi - 0xD800) << 10) + (c - 0xDC00), pos - 2));
        }
    }
    Some((c, pos - 1))
}

struct Matcher<'a> {
    prog: &'a Program,
    s: &'a [u16],
    unicode: bool,
    /// Pilhas de retrocesso reaproveitadas entre tentativas.
    pool: Vec<Vec<Bt>>,
    /// Posições finais das strings de uma classe (reaproveitado).
    ends: Vec<usize>,
}

impl Matcher<'_> {
    fn read_fwd(&self, pos: usize) -> Option<(u32, usize)> {
        decode_fwd(self.s, pos, self.unicode)
    }

    fn read_back(&self, pos: usize) -> Option<(u32, usize)> {
        decode_back(self.s, pos, self.unicode)
    }

    fn single_ok(&self, m: Single, c: u32) -> bool {
        match m {
            Single::Char(x) => c == x,
            Single::Any => true,
            Single::AnyNl => !(c <= 0xFFFF && is_line_term(c as u16)),
            Single::Class(i) => self.prog.classes[i as usize].contains(c),
        }
    }

    /// Consome um caractere que casa `m` (para frente ou para trás).
    fn step(&self, m: Single, pos: usize, back: bool) -> Option<usize> {
        let (c, np) = if back { self.read_back(pos)? } else { self.read_fwd(pos)? };
        if self.single_ok(m, c) { Some(np) } else { None }
    }

    fn is_word(&self, pos: usize, uicase: bool) -> bool {
        let Some(&c) = self.s.get(pos) else { return false };
        matches!(c, 0x30..=0x39 | 0x41..=0x5A | 0x5F | 0x61..=0x7A) || (uicase && (c == 0x17F || c == 0x212A))
    }

    /// Casa a string `t` (pontos de código) em `pos`; devolve onde termina.
    fn match_str(&self, t: &[u32], pos: usize, back: bool, icase: bool) -> Option<usize> {
        let mut p = pos;
        let fold = |c: u32| if icase { canon(c, true) } else { c };
        if back {
            for &x in t.iter().rev() {
                let (c, np) = self.read_back(p)?;
                if fold(c) != x {
                    return None;
                }
                p = np;
            }
        } else {
            for &x in t {
                let (c, np) = self.read_fwd(p)?;
                if fold(c) != x {
                    return None;
                }
                p = np;
            }
        }
        Some(p)
    }

    /// Fins possíveis de uma classe com strings em `pos`, na ordem do spec.
    fn string_ends(&self, ss: &StrSet, pos: usize, back: bool, out: &mut Vec<usize>) {
        if back {
            for &k in &ss.by_len {
                if let Some(e) = self.match_str(&ss.strs[k as usize], pos, true, ss.icase) {
                    out.push(e);
                }
            }
        } else if let Some((c, _)) = self.read_fwd(pos) {
            let k = if ss.icase { canon(c, true) } else { c };
            let i = ss.strs.partition_point(|s| s[0] < k);
            for t in ss.strs[i..].iter().take_while(|s| s[0] == k) {
                if let Some(e) = self.match_str(t, pos, false, ss.icase) {
                    out.push(e);
                }
            }
        }
        let one = if back { self.read_back(pos) } else { self.read_fwd(pos) };
        if let Some((c, np)) = one
            && ss.singles.contains(c)
        {
            out.push(np);
        }
        if ss.empty {
            out.push(pos);
        }
        out.dedup();
    }

    /// Compara a captura `s..e` em `pos`; devolve a nova posição.
    fn backref(&self, s: usize, e: usize, pos: usize, back: bool, icase: bool) -> Option<usize> {
        let text = self.s;
        if icase && self.unicode {
            // ponto de código a ponto de código, pela dobra
            let (mut a, mut b) = if back { (e, pos) } else { (s, pos) };
            loop {
                if (back && a == s) || (!back && a == e) {
                    return Some(b);
                }
                let (x, na) = if back { decode_back(text, a, true)? } else { decode_fwd(text, a, true)? };
                let (y, nb) = if back { decode_back(text, b, true)? } else { decode_fwd(text, b, true)? };
                if canon(x, true) != canon(y, true) {
                    return None;
                }
                a = na;
                b = nb;
            }
        }
        let len = e - s;
        let start = if back { pos.checked_sub(len)? } else { pos };
        if start + len > text.len() {
            return None;
        }
        let ok = (0..len).all(|k| {
            let (x, y) = (text[s + k] as u32, text[start + k] as u32);
            x == y || (icase && canon(x, false) == canon(y, false))
        });
        if !ok {
            return None;
        }
        // com u, não pode parar no meio de um par substituto
        if self.unicode && len > 0 {
            let lo = |i: usize| (0xDC00..0xE000).contains(&text[i]);
            let hi = |i: usize| (0xD800..0xDC00).contains(&text[i]);
            let end = start + len;
            if (!back && end < text.len() && hi(end - 1) && lo(end)) || (back && start > 0 && lo(start) && hi(start - 1)) {
                return None;
            }
        }
        Some(if back { start } else { pos + len })
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
                Inst::Strings { set, back } => {
                    let mut ends = std::mem::take(&mut self.ends);
                    ends.clear();
                    self.string_ends(&self.prog.strsets[set as usize], pos, back, &mut ends);
                    let ok = !ends.is_empty();
                    if ok {
                        // a mais longa primeiro; as outras ficam para o retrocesso
                        for &e in ends[1..].iter().rev() {
                            stack.push(Bt::Branch { pc: pc as u32 + 1, pos: e });
                        }
                        pos = ends[0];
                        pc += 1;
                    }
                    self.ends = ends;
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
                Inst::Start(ml) => {
                    pc += 1;
                    pos == 0 || (ml && is_line_term(self.s[pos - 1]))
                }
                Inst::End(ml) => {
                    pc += 1;
                    pos == self.s.len() || (ml && is_line_term(self.s[pos]))
                }
                Inst::WordB { neg, uicase } => {
                    let a = pos > 0 && self.is_word(pos - 1, uicase);
                    let b = self.is_word(pos, uicase);
                    pc += 1;
                    (a != b) != neg
                }
                Inst::BackRef { group, back, icase } => {
                    let (s, e) = (regs[2 * group as usize], regs[2 * group as usize + 1]);
                    pc += 1;
                    if s == NONE || e == NONE || e < s {
                        true
                    } else {
                        match self.backref(s, e, pos, back, icase) {
                            Some(np) => {
                                pos = np;
                                true
                            }
                            None => false,
                        }
                    }
                }
                Inst::BackRefAny { list, back, icase } => {
                    pc += 1;
                    let hit = self.prog.ref_lists[list as usize]
                        .iter()
                        .map(|&g| (regs[2 * g as usize], regs[2 * g as usize + 1]))
                        .find(|&(s, e)| s != NONE && e != NONE && e >= s);
                    match hit {
                        None => true,
                        Some((s, e)) => match self.backref(s, e, pos, back, icase) {
                            Some(np) => {
                                pos = np;
                                true
                            }
                            None => false,
                        },
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
    fn matcher<'a>(&'a self, s: &'a [u16]) -> Matcher<'a> {
        Matcher {
            prog: self,
            s,
            unicode: self.flags.full_unicode(),
            pool: Vec::new(),
            ends: Vec::new(),
        }
    }

    /// Tenta casar começando exatamente em `pos`. Devolve as capturas
    /// (pares início/fim; NONE = grupo não participou).
    pub fn match_at(&self, s: &[u16], pos: usize) -> Option<Vec<usize>> {
        let mut m = self.matcher(s);
        let mut regs = vec![NONE; self.nregs as usize];
        m.run(0, pos, &mut regs)?;
        regs.truncate(2 * (self.ngroups as usize + 1));
        Some(regs)
    }

    /// Procura a partir de `from` (ou só em `from`, se `sticky`).
    pub fn search(&self, s: &[u16], from: usize, sticky: bool) -> Option<Vec<usize>> {
        let unicode = self.flags.full_unicode();
        // com u, começar no meio de um par é começar no par inteiro
        let from = if unicode && mid_pair(s, from) { from - 1 } else { from };
        if sticky {
            return if from <= s.len() { self.match_at(s, from) } else { None };
        }
        if self.anchored {
            return if from == 0 { self.match_at(s, 0) } else { None };
        }
        let mut m = self.matcher(s);
        let mut regs = vec![NONE; self.nregs as usize];
        let mut pos = from;
        while pos <= s.len() {
            if let Some(f) = &self.first {
                if pos >= s.len() {
                    return None;
                }
                match f.find(s, pos) {
                    // a segunda metade de um par não é um começo possível
                    Some(k) if unicode && mid_pair(s, k) => {
                        pos = k + 1;
                        continue;
                    }
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

    /// Grupo que vale para o nome `k` de `names` (com nomes repetidos, o que participou).
    pub fn named_group(&self, k: usize, caps: &[usize]) -> u32 {
        let gs = &self.names[k].1;
        gs.iter()
            .copied()
            .find(|&g| caps.get(2 * g as usize).is_some_and(|&x| x != NONE))
            .unwrap_or(gs[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// Primeiro casamento e grupos como texto.
    fn m(p: &str, f: &str, s: &str) -> Option<Vec<Option<String>>> {
        let prog = compile(&u(p), Flags::parse(f).unwrap()).unwrap_or_else(|e| panic!("/{p}/: {}", e.0));
        let text = u(s);
        let r = prog.search(&text, 0, false)?;
        Some(
            r.chunks(2)
                .map(|c| if c[0] == NONE || c[1] == NONE { None } else { Some(String::from_utf16_lossy(&text[c[0]..c[1]])) })
                .collect(),
        )
    }

    fn whole(p: &str, f: &str, s: &str) -> Option<String> {
        m(p, f, s).and_then(|v| v[0].clone())
    }

    fn erro(p: &str, f: &str) -> String {
        let fl = Flags::parse(f).unwrap();
        let a = compile(&u(p), fl).err().map(|e| e.0);
        let b = validate(&u(p), fl).err().map(|e| e.0);
        assert_eq!(a, b, "/{p}/{f}: compile e validate discordam");
        a.unwrap_or_default()
    }

    #[test]
    fn basico() {
        assert_eq!(whole("b+", "", "abbbc"), Some("bbb".into()));
        assert_eq!(whole("b+?", "", "abbbc"), Some("b".into()));
        assert_eq!(whole("a.c", "", "xabcx"), Some("abc".into()));
        assert_eq!(whole("^abc$", "", "abc"), Some("abc".into()));
        assert_eq!(whole("^b", "m", "a\nb"), Some("b".into()));
        assert_eq!(whole("[a-c]{2,3}", "", "xxabcdx"), Some("abc".into()));
        assert_eq!(whole("\\d+(\\.\\d+)?", "", "v1.25x"), Some("1.25".into()));
        assert_eq!(whole("colou?r", "", "colour"), Some("colour".into()));
        assert_eq!(whole("ABC", "i", "xabcx"), Some("abc".into()));
        assert_eq!(whole("\\bfoo\\b", "", "a foo b"), Some("foo".into()));
        assert_eq!(whole("a|ab|abc", "", "abc"), Some("a".into()));
        assert_eq!(whole("(a|ab)(c|bcd)", "", "abcd"), Some("abcd".into()));
        assert_eq!(whole("x*", "", "aaa"), Some("".into()));
        assert_eq!(whole("[^abc]+", "", "abcdefa"), Some("def".into()));
        assert_eq!(whole("\\s+", "", "a \t\nb"), Some(" \t\n".into()));
        assert_eq!(whole(".*", "", "abc\ndef"), Some("abc".into()));
        assert_eq!(whole(".*", "s", "abc\ndef"), Some("abc\ndef".into()));
        assert_eq!(whole("a{3}", "", "aaaa"), Some("aaa".into()));
        assert_eq!(whole("(?:ab){2}", "", "abababx"), Some("abab".into()));
        assert_eq!(whole("(ab){1,2}?c", "", "ababc"), Some("ababc".into()));
        assert_eq!(whole("]", "", "a]"), Some("]".into()));
        assert_eq!(whole("a{,2}", "", "a{,2}"), Some("a{,2}".into()));
        assert_eq!(whole("\\x41\\u0042\\103", "", "ABC"), Some("ABC".into()));
    }

    #[test]
    fn grupos_e_referencias() {
        assert_eq!(m("(\\d+)-(\\d+)", "", "tel 12-345"), Some(vec![Some("12-345".into()), Some("12".into()), Some("345".into())]));
        assert_eq!(whole("(\\w)\\1", "", "abccd"), Some("cc".into()));
        assert_eq!(whole("(?<a>x)\\k<a>", "", "axxb"), Some("xx".into()));
        assert_eq!(m("(a)|(b)", "", "b"), Some(vec![Some("b".into()), None, Some("b".into())]));
        // capturas de iterações anteriores são limpas
        assert_eq!(m("(?:(a)|b)+", "", "ab"), Some(vec![Some("ab".into()), None]));
        assert_eq!(m("(z)((a+)?(b+)?(c))*", "", "zaacbbbcac"), Some(vec![
            Some("zaacbbbcac".into()),
            Some("z".into()),
            Some("ac".into()),
            Some("a".into()),
            None,
            Some("c".into()),
        ]));
        // laço vazio não trava
        assert_eq!(whole("(a*)*b", "", "aab"), Some("aab".into()));
        assert_eq!(whole("(?:a?)+?b", "", "b"), Some("b".into()));
    }

    #[test]
    fn nomes_repetidos() {
        assert_eq!(m("(?<x>a)|(?<x>b)", "", "b"), Some(vec![Some("b".into()), None, Some("b".into())]));
        assert_eq!(whole("(?:(?<x>a)|(?<x>b))\\k<x>", "", "xbb"), Some("bb".into()));
        let p = compile(&u("(?<x>a)|(?<y>c)|(?<x>b)"), Flags::default()).unwrap();
        assert_eq!(p.names, vec![("x".to_string(), vec![1, 3]), ("y".to_string(), vec![2])]);
        assert_eq!(erro("(?<a>x)(?<a>y)", ""), "Duplicate capture group name");
        assert_eq!(erro("((?<a>x)|(?<a>y))(?<a>z)", ""), "Duplicate capture group name");
        assert_eq!(erro("(?<a>.)\\k<b>", ""), "Invalid named capture referenced");
    }

    #[test]
    fn lookaround() {
        assert_eq!(whole("\\d+(?=px)", "", "10em 20px"), Some("20".into()));
        assert_eq!(whole("\\d+(?!px|\\d)", "", "20px 30em"), Some("30".into()));
        assert_eq!(whole("(?<=\\$)\\d+", "", "€10 $25"), Some("25".into()));
        assert_eq!(whole("(?<!\\$)\\b\\d+", "", "$10 25"), Some("25".into()));
        assert_eq!(m("(?<=(\\d+)(\\d+))$", "", "1053"), Some(vec![Some("".into()), Some("1".into()), Some("053".into())]));
        assert_eq!(m("(?=(\\w+))\\1:", "", "abc:"), Some(vec![Some("abc:".into()), Some("abc".into())]));
    }

    #[test]
    fn unicode() {
        assert_eq!(whole(".", "u", "😀x"), Some("😀".into()));
        assert_eq!(whole("^.$", "", "😀"), None);
        assert_eq!(whole("^.$", "u", "😀"), Some("😀".into()));
        assert_eq!(whole("\\u{1F600}", "u", "a😀"), Some("😀".into()));
        assert_eq!(whole("\\p{L}+", "u", "123 Olá!"), Some("Olá".into()));
        assert_eq!(whole("\\p{Script=Greek}+", "u", "abc αβγ"), Some("αβγ".into()));
        assert_eq!(whole("ÇÃO", "i", "ação"), Some("ção".into()));
        assert!(compile(&u("\\p{L}"), Flags::default()).is_ok());
        assert!(compile(&u("(a"), Flags::default()).is_err());
        assert!(compile(&u("a**"), Flags::default()).is_err());
        assert!(compile(&u("\\q"), Flags::parse("u").unwrap()).is_err());
        // sem u, o padrão é lido por unidades: o astral da classe são duas
        let p = compile(&u("[😀]"), Flags::default()).unwrap();
        assert!(p.search(&[0xD83D, 0x78], 0, false).is_some());
        let p = compile(&u("[😀]"), Flags::parse("u").unwrap()).unwrap();
        assert!(p.search(&[0xD83D, 0x78], 0, false).is_none());
    }

    #[test]
    fn propriedades() {
        assert_eq!(whole("\\p{RI}\\p{RI}", "u", "a🇧🇷b"), Some("🇧🇷".into()));
        assert_eq!(whole("\\p{Emoji}\\p{EMod}", "u", "x👍🏽y"), Some("👍🏽".into()));
        assert_eq!(whole("\\p{sc=Hira}+", "u", "カナ ひらがな"), Some("ひらがな".into()));
        assert_eq!(whole("\\p{scx=Hira}+", "u", "ー"), Some("ー".into()));
        assert_eq!(whole("\\P{Any}", "u", "abc"), None);
        assert_eq!(whole("[\\p{Lu}--[A-Z]]", "v", "ABCÀ"), Some("À".into()));
        assert_eq!(erro("\\p{Latin}", "u"), "Invalid property name");
        assert_eq!(erro("\\p{ascii}", "u"), "Invalid property name");
        assert_eq!(erro("\\p{ASCII=Yes}", "u"), "Invalid property name");
        assert_eq!(erro("[\\p{Foo}]", "u"), "Invalid property name in character class");
        assert_eq!(erro("\\p{RGI_Emoji}", "u"), "Invalid property name");
        assert_eq!(erro("\\P{RGI_Emoji}", "v"), "Invalid property name");
        // a regex da página inicial do GitHub
        let gh = "\\p{RI}\\p{RI}|\\p{Emoji}(\\p{EMod}|\\u{FE0F}\\u{20E3}?|[\\u{E0020}-\\u{E007E}]+\\u{E007F})?(\\u{200D}\\p{Emoji}(\\p{EMod}|\\u{FE0F}\\u{20E3}?|[\\u{E0020}-\\u{E007E}]+\\u{E007F})?)*";
        assert_eq!(whole(gh, "gu", "a 👨‍👩‍👧 b"), Some("👨‍👩‍👧".into()));
    }

    #[test]
    fn caixa() {
        assert_eq!(whole("[\\u0390]", "ui", "\u{1FD3}"), Some("\u{1FD3}".into()));
        assert_eq!(whole("\\u212A", "i", "k"), None);
        assert_eq!(whole("\\u212A", "iu", "k"), Some("k".into()));
        assert_eq!(whole("\\w", "iu", "\u{17F}"), Some("\u{17F}".into()));
        assert_eq!(whole("\\W", "iu", "\u{17F}"), None);
        assert_eq!(whole("\\W", "i", "\u{17F}"), Some("\u{17F}".into()));
        assert_eq!(whole("[^a]", "iv", "A"), None);
        assert_eq!(whole("\\P{Ll}", "iu", "a"), Some("a".into()));
        assert_eq!(whole("\\P{Ll}", "iv", "a"), None);
        assert_eq!(whole("(.)\\1", "iu", "\u{10400}\u{10428}"), Some("\u{10400}\u{10428}".into()));
        assert_eq!(whole("ß", "i", "ẞ"), None);
        assert_eq!(whole("ß", "iu", "ẞ"), Some("ẞ".into()));
    }

    #[test]
    fn flag_v() {
        assert_eq!(whole("[\\q{abc|d}]", "v", "xabc"), Some("abc".into()));
        assert_eq!(whole("^[\\p{RGI_Emoji_Flag_Sequence}]+$", "v", "🇧🇷🇵🇹"), Some("🇧🇷🇵🇹".into()));
        assert_eq!(whole("^\\p{RGI_Emoji}$", "v", "👨‍👩‍👧"), Some("👨‍👩‍👧".into()));
        assert_eq!(whole("[[a-z]--[aeiou]]+", "v", "aeixyzo"), Some("xyz".into()));
        assert_eq!(whole("[\\w&&\\d]+", "v", "ab12c"), Some("12".into()));
        assert_eq!(whole("[\\q{AB}]", "iv", "xab"), Some("ab".into()));
        assert_eq!(erro("[a-z&&b]", "v"), "Invalid set operation in character class");
        assert_eq!(erro("[(]", "v"), "Invalid character in character class");
        assert_eq!(erro("[^\\q{ab}]", "v"), "Negated character class may contain strings");
        assert_eq!(erro("[a!!b]", "v"), "Invalid set operation in character class");
        assert!(compile(&u("[^\\q{a}]"), Flags::parse("v").unwrap()).is_ok());
    }

    #[test]
    fn modificadores() {
        assert_eq!(whole("a(?i:b)c", "", "aBc"), Some("aBc".into()));
        assert_eq!(whole("a(?i:b)c", "", "aBC"), None);
        assert_eq!(whole("(?-i:a)b", "i", "aB"), Some("aB".into()));
        assert_eq!(whole("(?-i:a)b", "i", "AB"), None);
        assert_eq!(whole("(?m:^b)", "", "a\nb"), Some("b".into()));
        assert_eq!(whole("(?s:.)", "", "\n"), Some("\n".into()));
        assert_eq!(erro("(?ii:a)", ""), "Repeated flag in flag group");
        assert_eq!(erro("(?i-i:a)", ""), "Repeated flag in flag group");
        assert_eq!(erro("(?-:a)", ""), "Invalid flag group");
        assert_eq!(erro("(?x:a)", ""), "Invalid group");
    }

    #[test]
    fn texto_longo_sem_estourar_a_pilha() {
        let s: String = "a".repeat(200_000) + "b";
        assert_eq!(whole("a*b", "", &s).map(|x| x.len()), Some(200_001));
        assert_eq!(whole("(?:a|c)*b", "", &s).map(|x| x.len()), Some(200_001));
        let curto: String = "a".repeat(3000);
        assert_eq!(whole(".*x", "", &curto), None);
    }
}
