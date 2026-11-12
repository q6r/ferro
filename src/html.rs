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

