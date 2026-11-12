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

/// Endereço do ícone do site: `<link rel="icon">` (o maior até 64 px, de
/// preferência PNG/SVG) ou `/favicon.ico`.
pub fn favicon_url(doc: &Document, base: &Url) -> Option<Url> {
    if !matches!(base.scheme(), "http" | "https") {
        return None;
    }
    let mut best: Option<(i32, Url)> = None;
    for n in doc.find_all("link") {
        let Some(el) = doc.element(n) else { continue };
        let rel = el.attr("rel").unwrap_or("").to_ascii_lowercase();
        if !rel.split_whitespace().any(|r| r == "icon" || r == "apple-touch-icon") {
            continue;
        }
        let Some(u) = el.attr("href").and_then(|h| base.join(h.trim()).ok()) else { continue };
        let size: i32 = el
            .attr("sizes")
            .and_then(|s| s.split(['x', 'X']).next()?.trim().parse().ok())
            .unwrap_or(if rel.contains("apple") { 180 } else { 32 });
        // perto de 32 px é o ideal; svg vale como grande
        let svg = u.path().ends_with(".svg") || el.attr("type").is_some_and(|t| t.contains("svg"));
        let score = if svg { 1000 } else { 500 - (size - 32).abs() };
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, u));
        }
    }
    best.map(|(_, u)| u).or_else(|| base.join("/favicon.ico").ok())
}

/// Baixa e reduz o ícone para `px` × `px` (RGBA).
pub fn load_favicon(net: &Net, url: &Url, px: u32) -> Option<ImageData> {
    let res = net.fetch(url).ok().filter(|r| r.status < 400 && !r.body.is_empty())?;
    let ct = res.content_type.as_deref().unwrap_or("");
    let img = if ct.contains("svg") || url.path().ends_with(".svg") {
        let text = String::from_utf8_lossy(&res.body);
        crate::svg::parse(&text)?.render(px, px, None)
    } else {
        decode_image(&res.body)?
    };
    if img.width == 0 || img.height == 0 {
        return None;
    }
    // reduz pela média das áreas (fica nítido em 16-32 px)
    let (w, h) = (img.width as usize, img.height as usize);
    let n = px as usize;
    let mut out = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (x0, x1) = (x * w / n, ((x + 1) * w / n).max(x * w / n + 1).min(w));
            let (y0, y1) = (y * h / n, ((y + 1) * h / n).max(y * h / n + 1).min(h));
            let mut acc = [0u32; 4];
            let mut cnt = 0u32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = &img.rgba[(sy * w + sx) * 4..(sy * w + sx) * 4 + 4];
                    let a = p[3] as u32;
                    for k in 0..3 {
                        acc[k] += p[k] as u32 * a;
                    }
                    acc[3] += a;
                    cnt += 1;
                }
            }
            let o = (y * n + x) * 4;
            if acc[3] > 0 {
                for k in 0..3 {
                    out[o + k] = (acc[k] / acc[3]) as u8;
                }
            }
            out[o + 3] = (acc[3] / cnt.max(1)) as u8;
        }
    }
    Some(ImageData { width: px, height: px, rgba: out })
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

/// Baixa, com 8 threads, as imagens dos <img>, as imagens de fundo do CSS e
/// as fontes web (que vão para o catálogo global). `cancel` interrompe cedo
/// (quando o usuário já navegou para outra página).
pub fn fetch_resources(
    net: &Net,
    nodes: Vec<(NodeId, Url)>,
    bg_urls: Vec<String>,
    font_faces: Vec<FontFaceRule>,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Images {
    // a mesma URL aparece várias vezes (ícones repetidos): baixa uma vez só
    let mut by_url: HashMap<Url, Vec<NodeId>> = HashMap::new();
    for (n, u) in nodes {
        by_url.entry(u).or_default().push(n);
    }
    let mut jobs: Vec<Job> = by_url.into_iter().map(|(u, n)| Job::Img(u, n)).collect();
    jobs.extend(bg_urls.into_iter().map(Job::Bg));
    // fontes primeiro (são as últimas a sair da pilha... então vão no fim)
    jobs.extend(font_faces.into_iter().map(Job::Font));
    let queue = Mutex::new(jobs);
    let result = Mutex::new(Images::new());
    let decode = |bytes: &[u8]| decode_image(bytes).map(Arc::new);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                loop {
                    if cancel() {
                        return;
                    }
                    let Some(job) = queue.lock().unwrap().pop() else { return };
                    match job {
                        Job::Img(url, nodes) => {
                            let Ok(res) = net.fetch(&url) else { continue };
                            if res.status >= 400 {
                                continue;
                            }
                            if let Some(img) = decode(&res.body) {
                                let mut r = result.lock().unwrap();
                                for n in nodes {
                                    r.nodes.insert(n, img.clone());
                                }
                            }
                        }
                        Job::Bg(u) => {
                            let Ok(url) = Url::parse(&u) else { continue };
                            let Ok(res) = net.fetch(&url) else { continue };
                            if res.status >= 400 {
                                continue;
                            }
                            if let Some(img) = decode(&res.body) {
                                result.lock().unwrap().urls.insert(u, img);
                            }
                        }
                        Job::Font(face) => {
                            for src in &face.sources {
                                if fonts::web_font_tried(src) {
                                    break;
                                }
                                let Ok(url) = Url::parse(src) else { continue };
                                let Ok(res) = net.fetch(&url) else { continue };
                                if res.status < 400
                                    && res.body.len() < 4 * 1024 * 1024
                                    && fonts::register_web_font(src, &face.family, face.weight, face.italic, &res.body)
                                {
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }
    });
    result.into_inner().unwrap()
}

// ---------------------------------------------------- thread de renderização

/// Uma página pronta para a interface pintar.
pub struct Frame {
    pub url: Url,
    pub base: Url,
    pub title: String,
    pub status: u16,
    pub items: Vec<DisplayItem>,
    pub height: f32,
    pub background: Color,
    pub links: Vec<(Rect, String)>,
    /// Links dentro de `position: fixed` (coordenadas da janela).
    pub fixed_links: Vec<(Rect, String)>,
    pub anchors: Vec<(String, f32)>,
    /// Largura (em px CSS) para a qual o layout foi feito.
    pub width: f32,
    /// false enquanto as imagens ainda estão chegando.
    pub complete: bool,
    /// Quadro atual de cada `<video>`.
    pub videos: HashMap<NodeId, Arc<ImageData>>,
    /// Elementos que rolam por dentro (os mais internos primeiro).
    pub scrollers: Vec<layout::Scroller>,
    /// `<video controls>` parados.
    pub paused: std::collections::HashSet<NodeId>,
}

pub enum Command {
    Navigate { nav: u64, url: Url, post: Option<crate::script::PostData> },
    Resize { width: f32, height: f32 },
    /// Clique na página (coordenadas do documento, px CSS).
    Click { nav: u64, x: f32, y: f32 },
    /// A interface rolou a página.
    Scroll { nav: u64, y: f32 },
    /// Roda do mouse sobre um elemento que rola por dentro.
    ScrollElement { nav: u64, node: NodeId, dx: f32, dy: f32 },
    /// O mouse andou sobre a página (coordenadas do documento) ou saiu (None).
    MouseMove { nav: u64, point: Option<(f32, f32)> },
    /// A interface saiu da tela cheia (Esc).
    ExitFullscreen { nav: u64 },
    /// Opção escolhida na lista de um `<select>`.
    SelectChoose { nav: u64, node: NodeId, index: usize },
    /// Tecla (valor `key` do DOM e o texto digitado, se houver).
    Key { nav: u64, key: String, text: Option<String>, ctrl: bool, shift: bool, alt: bool, meta: bool },
    Quit,
}

pub enum Update {
    Started { nav: u64, url: Url },
    Frame { nav: u64, frame: Box<Frame> },
    Failed { nav: u64, url: Url, error: String },
    /// A página pediu para ir a outro endereço (link, location.href, formulário).
    Navigate { nav: u64, url: Url, post: Option<crate::script::PostData> },
    /// Um campo de texto ganhou ou perdeu o foco.
    Editing { nav: u64, editing: bool },
    /// A página pediu para rolar a janela até `y`.
    ScrollTo { nav: u64, y: f32 },
    /// Quadros novos de vídeo (nó do `<video>` → imagem).
    VideoFrames { nav: u64, frames: Vec<(NodeId, Arc<ImageData>)> },
    /// Ponteiro do mouse pedido pela página (CSS `cursor`).
    Cursor { nav: u64, cursor: crate::style::Cursor },
    /// A página entrou (true) ou saiu da tela cheia.
    Fullscreen { nav: u64, on: bool },
    /// Um `<select>` foi clicado: mostrar a lista.
    SelectPopup { nav: u64, popup: crate::script::SelectPopup },
    /// Ícone do site (favicon), já no tamanho da aba.
    Favicon { nav: u64, icon: Arc<ImageData> },
    /// A navegação era um arquivo: foi salvo (ou deu erro). `keep` é a
    /// navegação da página que continua aberta, se houver.
    Downloaded { nav: u64, keep: Option<u64>, name: String, result: Result<std::path::PathBuf, String> },
}

/// O lado da interface de uma thread de renderização.
pub struct EngineHandle {
    tx: mpsc::Sender<Command>,
    latest: Arc<AtomicU64>,
}

impl EngineHandle {
    pub fn spawn(send: impl Fn(Update) + Send + Sync + 'static, width: f32, height: f32) -> EngineHandle {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(AtomicU64::new(0));
        let l = latest.clone();
        std::thread::Builder::new()
            .name("ferro-render".into())
            .stack_size(crate::script::STACK_SIZE)
            .spawn(move || worker(rx, l, send, width, height))
            .expect("não foi possível criar a thread de renderização");
        EngineHandle { tx, latest }
    }

    pub fn navigate(&self, nav: u64, url: Url) {
        self.navigate_post(nav, url, None);
    }

    pub fn navigate_post(&self, nav: u64, url: Url, post: Option<crate::script::PostData>) {
        self.latest.store(nav, Ordering::SeqCst);
        let _ = self.tx.send(Command::Navigate { nav, url, post });
    }

    pub fn resize(&self, width: f32, height: f32) {
        let _ = self.tx.send(Command::Resize { width, height });
    }

    pub fn click(&self, nav: u64, x: f32, y: f32) {
        let _ = self.tx.send(Command::Click { nav, x, y });
    }

    pub fn scroll(&self, nav: u64, y: f32) {
        let _ = self.tx.send(Command::Scroll { nav, y });
    }

    pub fn scroll_element(&self, nav: u64, node: NodeId, dx: f32, dy: f32) {
        let _ = self.tx.send(Command::ScrollElement { nav, node, dx, dy });
    }

    pub fn mouse_move(&self, nav: u64, point: Option<(f32, f32)>) {
        let _ = self.tx.send(Command::MouseMove { nav, point });
    }

    pub fn exit_fullscreen(&self, nav: u64) {
        let _ = self.tx.send(Command::ExitFullscreen { nav });
    }

    pub fn select_choose(&self, nav: u64, node: NodeId, index: usize) {
        let _ = self.tx.send(Command::SelectChoose { nav, node, index });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn key(&self, nav: u64, key: String, text: Option<String>, ctrl: bool, shift: bool, alt: bool, meta: bool) {
        let _ = self.tx.send(Command::Key { nav, key, text, ctrl, shift, alt, meta });
    }

    /// Cancela a navegação em andamento.
    pub fn stop(&self) {
        self.latest.fetch_add(1 << 40, Ordering::SeqCst);
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.stop();
        let _ = self.tx.send(Command::Quit);
    }
}

fn make_frame(loaded: &Loaded, page: Page, width: f32, complete: bool) -> Box<Frame> {
    Box::new(Frame {
        url: loaded.url.clone(),
        base: loaded.base.clone(),
        title: loaded.current_title(),
        status: loaded.status,
        items: page.items,
        height: page.height,
        background: page.background,
        links: page.links,
        fixed_links: page.fixed_links,
        anchors: page.anchors,
        width,
        complete,
        videos: HashMap::new(),
        scrollers: page.scrollers,
        paused: Default::default(),
    })
}

/// A página aberta numa aba: a sessão JS e o estado do carregamento.
struct Current {
    nav: u64,
    session: crate::script::Session,
    /// Ainda carregando recursos (o `load` da janela não disparou).
    loading: bool,
    started: std::time::Instant,
    /// Último estado de foco avisado à interface.
    editing: bool,
    /// Último ponteiro avisado à interface.
    cursor: crate::style::Cursor,
}

/// Desenha a sessão e manda o quadro para a interface.
fn send_frame(cur: &mut Current, send: &impl Fn(Update), width: f32) {
    let page = cur.session.render();
    let complete = !cur.loading;
    let mut frame = make_frame(&cur.session.host.borrow().page, page, width, complete);
    frame.videos = cur.session.video_frames();
    frame.paused = cur.session.paused_videos();
    send(Update::Frame { nav: cur.nav, frame });
}

fn worker(
    rx: mpsc::Receiver<Command>,
    latest: Arc<AtomicU64>,
    send: impl Fn(Update) + Send + Sync + 'static,
    mut width: f32,
    mut height: f32,
) {
    // compartilhado com as threads que apresentam vídeo
    let send = Arc::new(send);
    let net = Net::new();
    let fonts = match Fonts::system() {
        Ok(f) => std::rc::Rc::new(f),
        Err(e) => {
            eprintln!("[ferro] {e}");
            return;
        }
    };
    let mut current: Option<Current> = None;

    loop {
        // espera um comando ou o próximo trabalho da página (timer, rede...)
        let wait = current.as_ref().and_then(|c| {
            let w = c.session.wakeup();
            if c.loading { Some(w.unwrap_or(Duration::from_millis(50)).min(Duration::from_millis(50))) } else { w }
        });
        let first = match wait {
            Some(d) => match rx.recv_timeout(d) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            },
        };
        // junta os comandos acumulados: vale a última navegação e o último tamanho
        let mut nav = None;
        let mut resized = false;
        let mut clicks = Vec::new();
        let mut keys = Vec::new();
        let mut scroll = None;
        let mut el_scrolls: Vec<(u64, NodeId, f32, f32)> = Vec::new();
        let mut mouse = None;
        let mut exit_fs = None;
        let mut chosen: Vec<(u64, NodeId, usize)> = Vec::new();
        for cmd in first.into_iter().chain(rx.try_iter()) {
            match cmd {
                Command::Navigate { nav: n, url, post } => nav = Some((n, url, post)),
                Command::Resize { width: w, height: h } => {
                    resized = resized || (w - width).abs() > 0.5 || (h - height).abs() > 0.5;
                    width = w;
                    height = h;
                }
                Command::Click { nav: n, x, y } => clicks.push((n, x, y)),
                Command::Scroll { nav: n, y } => scroll = Some((n, y)),
                Command::ScrollElement { nav: n, node, dx, dy } => el_scrolls.push((n, node, dx, dy)),
                // vale só a posição mais recente
                Command::MouseMove { nav: n, point } => mouse = Some((n, point)),
                Command::ExitFullscreen { nav: n } => exit_fs = Some(n),
                Command::SelectChoose { nav: n, node, index } => chosen.push((n, node, index)),
                Command::Key { nav: n, key, text, ctrl, shift, alt, meta } => {
                    keys.push((n, key, text, ctrl, shift, alt, meta))
                }
                Command::Quit => return,
            }
        }

        if let Some((n, url, post)) = nav {
            let stale = || latest.load(Ordering::SeqCst) != n;
            if stale() {
                continue;
            }
            send(Update::Started { nav: n, url: url.clone() });
            let loaded = match fetch_page(&net, &url, post.as_ref()) {
                Ok(Fetched::Page(l)) => *l,
                Ok(Fetched::Download(d)) => {
                    // um arquivo: salva e a página atual continua
                    let result = save_download(&d);
                    send(Update::Downloaded { nav: n, keep: current.as_ref().map(|c| c.nav), name: d.name, result });
                    continue;
                }
                Err(error) => {
                    current = None;
                    if !stale() {
                        send(Update::Failed { nav: n, url, error });
                    }
                    continue;
                }
            };
            // a página anterior (e o que ela ainda estava baixando) vai embora
            current = None;
            if stale() {
                continue;
            }
            let mut session = crate::script::Session::new(loaded, net.clone(), fonts.clone(), width, height);
            let s2 = send.clone();
            session.set_frame_sink(Arc::new(move |node, img| {
                s2(Update::VideoFrames { nav: n, frames: vec![(node, img)] });
            }));
            let mut cur = Current {
                nav: n,
                session,
                loading: true,
                started: std::time::Instant::now(),
                editing: false,
                cursor: crate::style::Cursor::Auto,
            };
            // primeiro desenho com o HTML como veio; os scripts rodam aos
            // poucos no laço abaixo (a aba continua respondendo)
            send_frame(&mut cur, &*send, width);
            // o ícone do site, numa thread à parte
            if let Some(u) = {
                let hb = cur.session.host.borrow();
                favicon_url(&hb.page.doc, &hb.page.base)
            } {
                let (net, send) = (net.clone(), send.clone());
                std::thread::spawn(move || {
                    if let Some(icon) = load_favicon(&net, &u, 32) {
                        send(Update::Favicon { nav: n, icon: Arc::new(icon) });
                    }
                });
            }
            cur.session.start_scripts();
            current = Some(cur);
            continue;
        }

        let Some(cur) = current.as_mut() else { continue };
        if resized {
            cur.session.resize(width, height);
        }
        if let Some((sn, y)) = scroll
            && sn == cur.nav
        {
            cur.session.set_scroll(y);
        }
        for (sn, node, dx, dy) in el_scrolls {
            if sn == cur.nav {
                cur.session.scroll_element(node, dx, dy);
            }
        }
        if exit_fs == Some(cur.nav) {
            cur.session.exit_fullscreen();
        }
        for (n, node, index) in chosen {
            if n == cur.nav {
                cur.session.choose_select(node, index);
            }
        }
        if let Some((mn, point)) = mouse
            && mn == cur.nav
        {
            let cursor = cur.session.mouse_move(point);
            if cursor != cur.cursor {
                cur.cursor = cursor;
                send(Update::Cursor { nav: cur.nav, cursor });
            }
        }
        let interacted = !clicks.is_empty() || !keys.is_empty();
        for (cn, x, y) in clicks {
            if cn == cur.nav
                && let Some(go) = cur.session.click(x, y)
            {
                send(Update::Navigate { nav: cur.nav, url: go.url, post: go.post });
            }
        }
        for (kn, key, text, ctrl, shift, alt, meta) in keys {
            if kn == cur.nav {
                cur.session.key(&key, text.as_deref(), ctrl, shift, alt, meta);
            }
        }
        if interacted {
            let editing = cur.session.editing();
            if editing != cur.editing {
                cur.editing = editing;
                send(Update::Editing { nav: cur.nav, editing });
            }
        }
        // scripts do HTML: um pedaço por volta
        if cur.session.scripts_pending() {
            cur.session.step_scripts(Duration::from_millis(80));
        }
        cur.session.tick();
        // (os quadros de vídeo vão direto da thread de apresentação)
        let _ = cur.session.take_new_frames();
        // canvas redesenhados: só a imagem nova, sem refazer o layout
        let frames = cur.session.take_canvas_frames();
        if !frames.is_empty() {
            send(Update::VideoFrames { nav: cur.nav, frames });
        }
        if let Some(y) = cur.session.take_page_scroll() {
            send(Update::ScrollTo { nav: cur.nav, y });
        }
        if let Some(on) = cur.session.take_fullscreen() {
            send(Update::Fullscreen { nav: cur.nav, on });
        }
        if let Some(popup) = cur.session.take_select_popup() {
            send(Update::SelectPopup { nav: cur.nav, popup });
        }
        if let Some(go) = cur.session.take_navigation() {
            send(Update::Navigate { nav: cur.nav, url: go.url, post: go.post });
        }
        // fim do carregamento: recursos chegaram (ou cansou de esperar)
        if cur.loading
            && (cur.session.idle() || (!cur.session.scripts_pending() && cur.started.elapsed() > Duration::from_secs(20)))
        {
            cur.loading = false;
            cur.session.finish_load();
            send_frame(cur, &*send, width);
            continue;
        }
        if resized || cur.session.wants_frame() {
            send_frame(cur, &*send, width);
        }
    }
}
