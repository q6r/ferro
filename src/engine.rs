//! Motor: o pipeline completo de uma página (rede → DOM → CSS → layout) e a
//! thread de renderização de cada aba, que conversa com a interface por
//! mensagens, no espírito do processo de renderização do Chrome.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use url::Url;

use crate::css::{self, FontFaceRule, MediaContext};
use crate::dom::{Document, NodeId};
use crate::fonts::{self, Fonts};
use crate::layout::{self, Images, Page, Rect};
use crate::net::{self, Net};
use crate::paint::{DisplayItem, ImageData};
use crate::style::{self, Color, Origin};
use crate::{UA_STYLESHEET, html};

/// Folha de estilo do autor, ainda em texto (o atributo `media` é avaliado
/// a cada layout, porque depende da largura da janela).
pub struct StyleSource {
    pub text: String,
    pub media: Option<String>,
}

/// Documento carregado: DOM, folhas de estilo e URLs.
pub struct Loaded {
    pub url: Url,
    pub base: Url,
    pub status: u16,
    pub doc: Document,
    pub css: Vec<StyleSource>,
    pub title: String,
    /// Texto das folhas externas já baixadas (None = falhou), por URL.
    pub sheet_texts: HashMap<Url, Option<String>>,
}

impl Loaded {
    /// Título atual do documento (os scripts podem mudar).
    pub fn current_title(&self) -> String {
        self.doc
            .find_first("title")
            .map(|t| self.doc.text_content(t).split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Baixa o documento e as folhas de estilo dele.
pub fn load(net: &Net, url: &Url) -> Result<Loaded, String> {
    load_with(net, url, None)
}

/// Um arquivo que não é página: vai para a pasta de downloads.
pub struct Download {
    pub url: Url,
    pub name: String,
    pub bytes: Vec<u8>,
}

/// O que veio de uma navegação.
pub enum Fetched {
    Page(Box<Loaded>),
    Download(Download),
}

/// Nome do arquivo pelo `Content-Disposition` (filename* ou filename) ou pela URL.
fn download_name(res: &net::Response) -> String {
    let cd = res.headers.iter().find(|(k, _)| k == "content-disposition").map(|(_, v)| v.as_str()).unwrap_or("");
    let mut name = None;
    for part in cd.split(';') {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"'));
        if k == "filename*" {
            // UTF-8''nome%20codificado
            let enc = v.rsplit("''").next().unwrap_or(v);
            name = Some(String::from_utf8_lossy(&net::percent_decode(enc)).into_owned());
            break;
        }
        if k == "filename" && !v.is_empty() {
            name = Some(v.to_string());
        }
    }
    let name = name.unwrap_or_else(|| {
        let last = res.url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
        let last = String::from_utf8_lossy(&net::percent_decode(last)).into_owned();
        if last.is_empty() { "download".to_string() } else { last }
    });
    // sem caminhos nem caracteres proibidos no Windows
    let clean: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() { "download".into() } else { clean }
}

/// A resposta deve virar download (e não página)?
fn is_download(res: &net::Response) -> bool {
    let cd = res.headers.iter().find(|(k, _)| k == "content-disposition").map(|(_, v)| v.to_ascii_lowercase());
    if cd.is_some_and(|v| v.trim_start().starts_with("attachment")) {
        return true;
    }
    let ct = res.content_type.as_deref().unwrap_or("").to_ascii_lowercase();
    let ct = ct.split(';').next().unwrap_or("").trim().to_string();
    if ct.is_empty() {
        // sem tipo: texto vira página, binário vira download
        return res.body.iter().take(512).any(|&b| b == 0);
    }
    !(ct.starts_with("text/")
        || ct.starts_with("image/")
        || ct.contains("html")
        || ct.contains("xml")
        || ct.contains("json")
        || ct.contains("javascript"))
}

/// Salva um download na pasta Downloads do usuário (sem sobrescrever nada).
pub fn save_download(d: &Download) -> Result<std::path::PathBuf, String> {
    let dir = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| std::path::PathBuf::from(h).join("Downloads"))
        .ok_or("pasta de downloads não encontrada")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (stem, ext) = match d.name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (d.name.clone(), String::new()),
    };
    let mut path = dir.join(&d.name);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&path, &d.bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Busca uma navegação: página ou download.
pub fn fetch_page(net: &Net, url: &Url, post: Option<&crate::script::PostData>) -> Result<Fetched, String> {
    let res = request_page(net, url, post)?;
    if res.status < 400 && is_download(&res) {
        let name = download_name(&res);
        return Ok(Fetched::Download(Download { url: res.url, name, bytes: res.body }));
    }
    Ok(Fetched::Page(Box::new(page_from_response(net, res))))
}

fn request_page(net: &Net, url: &Url, post: Option<&crate::script::PostData>) -> Result<net::Response, String> {
    Ok(match post {
        None => net.fetch(url)?,
        Some((ct, body)) => {
            let mut req = net::Request::get(url);
            req.method = "POST".into();
            req.headers.push(("Content-Type".into(), ct.clone()));
            req.headers.push(("Origin".into(), url.origin().ascii_serialization()));
            req.body = Some(body.clone());
            net.request(&req)?
        }
    })
}

/// Documento de um `<iframe>`: vai com o `Referer` da página que o contém
/// (players embutidos, como o do YouTube, exigem).
pub fn load_frame(net: &Net, url: &Url, referrer: Option<&Url>) -> Result<Loaded, String> {
    let mut req = net::Request::get(url);
    if let Some(r) = referrer.filter(|r| matches!(r.scheme(), "http" | "https")) {
        // só a origem quando muda de site (política padrão dos navegadores)
        let value = if r.origin() == url.origin() { r.to_string() } else { format!("{}/", r.origin().ascii_serialization()) };
        req.headers.push(("Referer".into(), value));
    }
    let res = net.request(&req)?;
    Ok(page_from_response(net, res))
}

/// Como `load`, opcionalmente enviando um formulário por POST.
pub fn load_with(net: &Net, url: &Url, post: Option<&crate::script::PostData>) -> Result<Loaded, String> {
    let res = request_page(net, url, post)?;
    Ok(page_from_response(net, res))
}

fn page_from_response(net: &Net, res: net::Response) -> Loaded {
    let ct = res.content_type.as_deref().unwrap_or("").to_ascii_lowercase();
    let text = if ct.starts_with("image/") {
        format!(
            "<html><head><title>{0}</title></head><body style=\"margin:0;background:#0e0e0e;text-align:center\"><img src=\"{0}\" style=\"max-width:100%\"></body></html>",
            escape_html(res.url.as_str())
        )
    } else if ct.starts_with("text/plain") || (ct.is_empty() && res.url.path().ends_with(".txt")) {
        format!(
            "<pre style=\"white-space:pre-wrap\">{}</pre>",
            escape_html(&net::decode_text(&res.body, res.content_type.as_deref(), false))
        )
    } else {
        net::decode_text(&res.body, res.content_type.as_deref(), true)
    };
    let doc = html::parse(&text);
    let base = base_url(&doc, &res.url);
    let title = doc
        .find_first("title")
        .map(|t| doc.text_content(t).split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let (css, sheet_texts) = fetch_stylesheets(&doc, &base, net);
    Loaded {
        url: res.url,
        base,
        status: res.status,
        doc,
        css,
        title,
        sheet_texts,
    }
}

pub fn base_url(doc: &Document, page: &Url) -> Url {
    doc.find_first("base")
        .and_then(|b| doc.element(b)?.attr("href"))
        .and_then(|h| page.join(h.trim()).ok())
        .unwrap_or_else(|| page.clone())
}

enum Source {
    Inline(String),
    Link(Url),
}

/// Troca os `url(...)` relativos do CSS por endereços absolutos (as URLs de
/// uma folha externa são relativas a ela, não à página).
pub fn absolutize_css_urls(css: &str, base: &Url) -> String {
    let lower = css.to_ascii_lowercase();
    let mut out = String::with_capacity(css.len() + 64);
    let mut i = 0;
    while let Some(off) = lower[i..].find("url(") {
        let start = i + off + 4;
        out.push_str(&css[i..start]);
        let rest = &css[start..];
        let lead = rest.len() - rest.trim_start().len();
        let body = rest.trim_start();
        // fim do url(): depois da aspa de fechamento, se houver
        let end = match body.chars().next() {
            Some(q @ ('"' | '\'')) => body[1..].find(q).and_then(|e| body[e + 2..].find(')').map(|p| e + 2 + p)),
            _ => body.find(')'),
        };
        let Some(end) = end else {
            i = start;
            break;
        };
        let raw = body[..end].trim().trim_matches(|c| c == '"' || c == '\'');
        if raw.is_empty() || raw.starts_with("data:") || raw.starts_with('#') {
            out.push_str(&rest[..lead + end]);
        } else if let Ok(u) = base.join(raw) {
            out.push('"');
            out.push_str(u.as_str());
            out.push('"');
        } else {
            out.push_str(&rest[..lead + end]);
        }
        i = start + lead + end;
    }
    out.push_str(&css[i..]);
    out
}

/// Os <style> e os <link rel=stylesheet>, na ordem do documento. As folhas
/// externas são baixadas em paralelo.
fn fetch_stylesheets(doc: &Document, base: &Url, net: &Net) -> (Vec<StyleSource>, HashMap<Url, Option<String>>) {
    let mut sources = Vec::new();
    for n in doc.descendants(Document::ROOT) {
        let Some(el) = doc.element(n) else { continue };
        let media = el.attr("media").map(str::to_string);
        match el.tag.as_str() {
            "style" => sources.push((Source::Inline(doc.text_content(n)), media)),
            "link" => {
                let rel = el.attr("rel").unwrap_or("").to_ascii_lowercase();
                let rels: Vec<&str> = rel.split_whitespace().collect();
                if rels.contains(&"stylesheet")
                    && !rels.contains(&"alternate")
                    && let Some(href) = el.attr("href")
                    && let Ok(u) = base.join(href.trim())
                {
                    sources.push((Source::Link(u), media));
                }
            }
            _ => {}
        }
    }

    let fetched: Vec<Option<String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = sources
            .iter()
            .map(|(s, _)| match s {
                Source::Link(u) => {
                    let net = net.clone();
                    let u = u.clone();
                    Some(scope.spawn(move || match net.fetch(&u) {
                        Ok(r) if r.status < 400 => Some(net::decode_text(&r.body, r.content_type.as_deref(), false)),
                        _ => None,
                    }))
                }
                Source::Inline(_) => None,
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.and_then(|h| h.join().ok().flatten()))
            .collect()
    });

    let mut texts = HashMap::new();
    let list = sources
        .into_iter()
        .zip(fetched)
        .filter_map(|((src, media), text)| {
            let text = match src {
                Source::Inline(t) => absolutize_css_urls(&t, base),
                Source::Link(u) => {
                    texts.insert(u.clone(), text.clone());
                    absolutize_css_urls(&text?, &u)
                }
            };
            Some(StyleSource { text, media })
        })
        .collect();
    (list, texts)
}

/// Regras do modo quirks (HTML, seção "Rendering"): tabelas não herdam
/// alinhamento nem fonte de quem está em volta.
const QUIRKS_STYLESHEET: &str = "table { font-weight: normal; font-style: normal; font-size: medium; \
     line-height: normal; white-space: normal; text-align: left; }";

/// Sem <!DOCTYPE html>, o documento é renderizado em modo quirks.
pub fn is_quirks(doc: &Document) -> bool {
    !doc.node(Document::ROOT).children.iter().any(|&c| match &doc.node(c).data {
        crate::dom::NodeData::Doctype(d) => d.to_ascii_lowercase().starts_with("html"),
        _ => false,
    })
}

thread_local! {
    /// Folhas já analisadas (texto + tamanho da janela, por causa do @media).
    static SHEETS: std::cell::RefCell<HashMap<(u64, usize, u32, u32), std::rc::Rc<css::Stylesheet>>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Analisa a folha ou devolve a que já foi analisada (os scripts provocam
/// muitos redesenhos, e reanalisar todo o CSS a cada um custa caro).
fn cached_sheet(text: &str, media: &MediaContext) -> std::rc::Rc<css::Stylesheet> {
    use std::hash::Hasher;
    let mut h = ferro_js::fxhash::FxHasher::default();
    h.write(text.as_bytes());
    let key = (h.finish(), text.len(), media.width.to_bits(), media.height.to_bits());
    SHEETS.with(|c| {
        if let Some(s) = c.borrow().get(&key) {
            return s.clone();
        }
        let s = std::rc::Rc::new(css::parse_stylesheet(text, media));
        let mut c = c.borrow_mut();
        if c.len() > 200 {
            c.clear();
        }
        c.insert(key, s.clone());
        s
    })
}

/// Estilo + layout para uma largura de janela.
pub fn render(loaded: &Loaded, images: &Images, fonts: &Fonts, width: f32, height: f32) -> Page {
    render_styled(loaded, images, fonts, width, height).0
}

/// Como `render`, devolvendo também os estilos calculados (getComputedStyle).
pub fn render_styled(
    loaded: &Loaded,
    images: &Images,
    fonts: &Fonts,
    width: f32,
    height: f32,
) -> (Page, style::Styles) {
    let timing = std::env::var_os("FERRO_TEMPOS").is_some();
    let t0 = std::time::Instant::now();
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
    let media = MediaContext { width, height };
    let mut sheets = vec![(cached_sheet(UA_STYLESHEET, &media), Origin::UserAgent)];
    if is_quirks(&loaded.doc) {
        sheets.push((cached_sheet(QUIRKS_STYLESHEET, &media), Origin::UserAgent));
    }
    if html::scripting() {
        sheets.push((cached_sheet("noscript { display: none }", &media), Origin::UserAgent));
    }
    for s in &loaded.css {
        if s.media.as_deref().is_none_or(|m| css::media_matches(m, &media)) {
            sheets.push((cached_sheet(&s.text, &media), Origin::Author));
        }
    }
    let t_css = ms(t0);
    let t1 = std::time::Instant::now();
    let styles = style::compute(&loaded.doc, &sheets, &media);
    let t_cascade = ms(t1);
    let t2 = std::time::Instant::now();
    let root = layout::build_box_tree(&loaded.doc, &styles);
    let t_boxes = ms(t2);
    let t3 = std::time::Instant::now();
    let page = root.as_ref().map(|root| layout::layout(&loaded.doc, root, fonts, images, width, height));
    if timing {
        eprintln!(
            "[ferro] tempos: CSS {t_css:.0} ms · cascata {t_cascade:.0} ms · caixas {t_boxes:.0} ms · layout {:.0} ms",
            ms(t3)
        );
    }
    let mut page = match page {
        Some(page) => page,
        None => Page {
            items: Vec::new(),
            height,
            background: Color::WHITE,
            links: Vec::new(),
            boxes: Vec::new(),
            scrollers: Vec::new(),
            hits: Vec::new(),
            fixed_nodes: Default::default(),
            fixed_links: Vec::new(),
            anchors: Vec::new(),
            missing_urls: Vec::new(),
            missing_fonts: Vec::new(),
        },
    };
    // fontes web das famílias que a página realmente usa
    let used: std::collections::HashSet<&str> = styles
        .nodes
        .iter()
        .flatten()
        .flat_map(|s| s.font_names.iter().map(|n| &**n))
        .collect();
    let mut seen = std::collections::HashSet::new();
    page.missing_fonts = sheets
        .iter()
        .flat_map(|(sh, _)| &sh.font_faces)
        .filter(|f| f.latin && used.contains(f.family.as_str()))
        .filter(|f| !f.sources.iter().any(|u| fonts::web_font_tried(u)))
        .filter(|f| seen.insert(f.sources[0].clone()))
        .take(24)
        .cloned()
        .collect();
    (page, styles)
}

// ------------------------------------------------------------------ imagens

const MAX_IMAGES: usize = 200;
const MAX_IMAGE_SIDE: u32 = 4096;

/// URL de cada <img> (com suporte aos atributos de carregamento preguiçoso
/// mais comuns e ao primeiro candidato de `srcset`).
pub fn image_sources(doc: &Document, base: &Url) -> Vec<(NodeId, Url)> {
    let mut out = Vec::new();
    let imgs: Vec<NodeId> = if doc.shadow_roots.is_empty() {
        doc.find_all("img")
    } else {
        (0..doc.nodes.len()).filter(|&n| doc.tag(n) == Some("img") && doc.is_connected(n)).collect()
    };
    for n in imgs {
        let Some(el) = doc.element(n) else { continue };
        let lazy = el.attr("data-src").or(el.attr("data-lazy-src")).filter(|s| !s.trim().is_empty());
        let src = el.attr("src").filter(|s| !s.trim().is_empty());
        let srcset = || {
            el.attr("srcset")
                .or(el.attr("data-srcset"))
                .and_then(|s| s.split(',').next())
                .and_then(|c| c.split_whitespace().next())
        };
        let chosen = lazy.or(src).or_else(srcset);
        if let Some(u) = chosen.and_then(|s| base.join(s.trim()).ok()) {
            out.push((n, u));
        }
        if out.len() >= MAX_IMAGES {
            break;
        }
    }
    out
}

pub fn decode_image(bytes: &[u8]) -> Option<ImageData> {
    let Ok(mut img) = image::load_from_memory(bytes) else {
        // SVG, com o renderizador próprio
        let text = std::str::from_utf8(bytes).ok()?;
        if !text.contains("<svg") {
            return None;
        }
        let svg = crate::svg::parse(text)?;
        let w = svg.width.clamp(1.0, 2048.0).round() as u32;
        let h = svg.height.clamp(1.0, 2048.0).round() as u32;
        return Some(svg.render(w, h, None));
    };
    if img.width() > MAX_IMAGE_SIDE || img.height() > MAX_IMAGE_SIDE {
        img = img.thumbnail(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE);
    }
    let rgba = img.to_rgba8();
    Some(ImageData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Baixa e decodifica as imagens dos <img>.
pub fn fetch_images(net: &Net, sources: Vec<(NodeId, Url)>, cancel: &(dyn Fn() -> bool + Sync)) -> Images {
    fetch_resources(net, sources, Vec::new(), Vec::new(), cancel)
}

enum Job {
    Img(Url, Vec<NodeId>),
    Bg(String),
    Font(FontFaceRule),
}

