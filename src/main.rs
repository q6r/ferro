use std::path::PathBuf;
use std::time::Instant;

use ferro::engine;
use ferro::fonts::Fonts;
use ferro::layout;
use ferro::net::Net;
use ferro::paint::Canvas;
use url::Url;

/// Alocador próprio (objetos pequenos sem passar pelo heap do sistema).
#[global_allocator]
static ALLOC: ferro::alloc::Alloc = ferro::alloc::Alloc;

const HELP: &str = "\
Ferro 0.1: navegador com motor próprio, 100% Rust

Uso:
  ferro [url-ou-arquivo]              abre a janela do navegador
  ferro <url-ou-arquivo> -o pag.png   renderiza a página num PNG (sem janela)

Opções do modo PNG:
  -o, --saida <arquivo>   PNG de saída
  -w, --largura <px>      largura da página (padrão: 1280)
      --altura <px>       altura da janela, usada por media queries (padrão: 800)
      --max <px>          altura máxima do PNG (padrão: 12000)
      --sem-imagens       não baixa imagens
      --sem-js            não roda os scripts da página
      --espera <ms>       tempo máximo esperando scripts e recursos (padrão: 5000)
      --dom               imprime a árvore DOM
      --caixas            imprime a árvore de caixas

JavaScript (motor próprio, Ferro JS):
  ferro --js arquivo.js               executa um script
  ferro --js                          abre o console interativo (REPL)

Captura da interface (sem abrir janela):
      --captura <arquivo> salva a interface inteira num PNG
      --tema <id>         carvao, moca, crepusculo, nevoa ou claro
      --layout <id>       lateral ou capsulas
      --buscar <texto>    abre a busca na página (Ctrl+F) com o texto
      --selecionar-tudo   seleciona o texto da página (Ctrl+A)
      --rolar <px>        rola a página antes da captura
      --digitar <texto>   abre a paleta de busca com esse texto (vazio = só abre)
      --abas <a,b,...>    abre essas URLs em outras abas antes da principal
  -h, --help              mostra esta ajuda";

#[derive(Clone)]
struct Args {
    target: Option<String>,
    out: Option<PathBuf>,
    width: f32,
    height: f32,
    max_h: f32,
    images: bool,
    js: bool,
    wait_ms: f32,
    dump_dom: bool,
    dump_boxes: bool,
    capture: Option<PathBuf>,
    theme: Option<String>,
    layout: Option<String>,
    typed: Option<String>,
    find: Option<String>,
    select_all: bool,
    scroll: Option<f32>,
    extra_tabs: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        target: None,
        out: None,
        width: 1280.0,
        height: 800.0,
        max_h: 12000.0,
        images: true,
        js: true,
        wait_ms: 5000.0,
        dump_dom: false,
        dump_boxes: false,
        capture: None,
        theme: None,
        layout: None,
        typed: None,
        find: None,
        select_all: false,
        scroll: None,
        extra_tabs: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut num = |name: &str| -> Result<f32, String> {
            it.next()
                .and_then(|v| v.parse().ok())
                .filter(|v: &f32| *v > 0.0)
                .ok_or(format!("{name} espera um número positivo"))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            "-w" | "--largura" => a.width = num("--largura")?,
            "--altura" => a.height = num("--altura")?,
            "--max" => a.max_h = num("--max")?,
            "-o" | "--saida" => a.out = Some(PathBuf::from(it.next().ok_or("--saida espera um arquivo")?)),
            "--sem-imagens" => a.images = false,
            "--sem-js" => a.js = false,
            "--espera" => a.wait_ms = num("--espera")?,
            "--dom" => a.dump_dom = true,
            "--caixas" => a.dump_boxes = true,
            "--captura" => a.capture = Some(PathBuf::from(it.next().ok_or("--captura espera um arquivo")?)),
            "--tema" => a.theme = Some(it.next().ok_or("--tema espera um nome")?),
            "--digitar" => a.typed = Some(it.next().ok_or("--digitar espera um texto")?),
            "--buscar" => a.find = Some(it.next().ok_or("--buscar espera um texto")?),
            "--selecionar-tudo" => a.select_all = true,
            "--rolar" => a.scroll = Some(num("--rolar")?),
            "--layout" => a.layout = Some(it.next().ok_or("--layout espera lateral ou capsulas")?),
            "--abas" => {
                let list = it.next().ok_or("--abas espera URLs separadas por vírgula")?;
                a.extra_tabs = list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
            s if s.starts_with('-') => return Err(format!("opção desconhecida: {s}")),
            s => a.target = Some(s.to_string()),
        }
    }
    Ok(a)
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("--js") {
        let file = raw.get(1).cloned();
        // pilha grande: o interpretador é recursivo nas chamadas de função
        let code = std::thread::Builder::new()
            .stack_size(1 << 30)
            .spawn(move || run_js(file))
            .map(|h| h.join().unwrap_or(70))
            .unwrap_or(70);
        std::process::exit(code);
    }
    // FERRO_ESCURO=1: os sites veem prefers-color-scheme: dark (modo PNG)
    if std::env::var_os("FERRO_ESCURO").is_some() {
        ferro::css::set_prefers_dark(true);
    }
    // FERRO_DADOS=pasta: cookies e localStorage gravados nessa pasta (modo PNG)
    if let Some(dir) = std::env::var_os("FERRO_DADOS") {
        ferro::dados::enable(PathBuf::from(dir));
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{HELP}");
            std::process::exit(2);
        }
    };
    let headless = args.out.is_some() || args.dump_dom || args.dump_boxes;
    let result = if headless {
        render_png(&args)
    } else {
        let url = match args.target.as_deref().map(resolve_target).transpose() {
            Ok(u) => u,
            Err(e) => {
                eprintln!("erro: {e}");
                std::process::exit(2);
            }
        };
        let extra_tabs: Vec<Url> = args.extra_tabs.iter().filter_map(|t| resolve_target(t).ok()).collect();
        let capture = args.capture.clone().map(|out| ferro::ui::Capture {
            out,
            width: args.width,
            height: if args.height == 800.0 { 860.0 } else { args.height },
            theme: args.theme.clone(),
            layout: args.layout.clone(),
            typed: args.typed.clone(),
            extra_tabs,
            find: args.find.clone(),
            select_all: args.select_all,
            scroll: args.scroll,
        });
        ferro::ui::run(url, capture)
    };
    ferro::dados::flush();
    if let Err(e) = result {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
}

/// `ferro --js [arquivo]`: executa um script ou abre o REPL.
fn run_js(file: Option<String>) -> i32 {
    let mut js = ferro_js::Engine::new();
    js.vm.max_depth = 8000;
    ferro::jshost::setup(&mut js);
    match file {
        Some(path) => {
            let src = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("erro: não consegui ler {path}: {e}");
                    return 2;
                }
            };
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or(path.clone());
            let t = Instant::now();
            let r = js.eval(&src, &name);
            let code = match r {
                Ok(_) => 0,
                Err(e) => {
                    eprintln!("{}", js.error_text(&e));
                    1
                }
            };
            js.run_event_loop();
            if std::env::var("FERRO_TEMPOS").is_ok() {
                eprintln!("[ferro-js] {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
            }
            code
        }
        None => {
            use std::io::{BufRead, Write};
            println!("Ferro JS — console interativo. Digite .sair para sair.");
            let stdin = std::io::stdin();
            let mut buf = String::new();
            loop {
                print!("{}", if buf.is_empty() { "> " } else { "… " });
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                    println!();
                    return 0;
                }
                let trimmed = line.trim();
                if buf.is_empty() && (trimmed == ".sair" || trimmed == ".exit") {
                    return 0;
                }
                buf.push_str(&line);
                // entrada incompleta (bloco aberto): continua lendo
                if let Err(e) = ferro_js::parser::parse_program(&buf)
                    && e.eof
                {
                    continue;
                }
                let src = std::mem::take(&mut buf);
                match js.eval(&src, "repl") {
                    Ok(v) => println!("{}", js.inspect(&v)),
                    Err(e) => println!("{}", js.error_text(&e)),
                }
                js.run_due_timers();
            }
        }
    }
}

fn log(msg: impl AsRef<str>) {
    eprintln!("[ferro] {}", msg.as_ref());
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// URL, caminho de arquivo, ou domínio sem esquema (vira https://).
fn resolve_target(t: &str) -> Result<Url, String> {
    if let Ok(u) = Url::parse(t)
        && matches!(u.scheme(), "http" | "https" | "file" | "data" | "ferro")
    {
        return Ok(u);
    }
    let p = std::path::Path::new(t);
    if p.exists() {
        let abs = std::path::absolute(p).map_err(|e| e.to_string())?;
        return Url::from_file_path(&abs).map_err(|_| format!("caminho inválido: {t}"));
    }
    Url::parse(&format!("https://{t}")).map_err(|e| format!("{t}: {e}"))
}

fn render_png(a: &Args) -> Result<(), String> {
    // os scripts precisam de pilha grande (o interpretador é recursivo)
    let a = a.clone();
    std::thread::Builder::new()
        .stack_size(ferro::script::STACK_SIZE)
        .spawn(move || render_png_inner(&a))
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "a renderização falhou".to_string())?
}

/// Carrega a página numa sessão e roda o laço de eventos até ela sossegar.
fn open_session(
    a: &Args,
    net: &Net,
    fonts: &std::rc::Rc<Fonts>,
    go: &ferro::script::Nav,
) -> Result<ferro::script::Session, String> {
    let t = Instant::now();
    let url = &go.url;
    log(format!("carregando {url}{}", if go.post.is_some() { " (POST)" } else { "" }));
    let loaded = engine::load_with(net, url, go.post.as_ref())?;
    log(format!(
        "  HTTP {} · {} nós · {} folhas de estilo · {:.0} ms",
        loaded.status,
        loaded.doc.nodes.len(),
        loaded.css.len(),
        ms(t)
    ));
    let mut session = ferro::script::Session::new(loaded, net.clone(), fonts.clone(), a.width, a.height);
    if !a.images {
        session.disable_resources();
    }
    // primeiro desenho: pede imagens, fundos e fontes
    let _ = session.render();
    if a.js {
        let t = Instant::now();
        session.run_scripts(&|| false);
        log(format!("scripts: {:.0} ms", ms(t)));
    }
    let t = Instant::now();
    settle(&mut session, a, std::time::Duration::from_secs_f32(a.wait_ms / 1000.0), true);
    log(format!("laço de eventos: {:.0} ms", ms(t)));
    Ok(session)
}

/// Laço de eventos até a página sossegar (ou o tempo acabar).
fn settle(session: &mut ferro::script::Session, a: &Args, budget: std::time::Duration, first: bool) {
    let t = Instant::now();
    let mut loaded_fired = !first;
    let mut quiet_since: Option<Instant> = None;
    while t.elapsed() < budget {
        session.tick();
        if session.wants_frame() {
            let _ = session.render();
        }
        if session.idle() {
            if !loaded_fired && a.js {
                session.finish_load();
                loaded_fired = true;
                continue;
            }
            // dá um tempo para timers curtos (setTimeout 0, animações de entrada)
            let q = *quiet_since.get_or_insert_with(Instant::now);
            if q.elapsed().as_millis() > 300 || !a.js {
                break;
            }
        } else {
            quiet_since = None;
        }
        let nap = session.wakeup().unwrap_or(std::time::Duration::from_millis(20));
        std::thread::sleep(nap.min(std::time::Duration::from_millis(20)));
    }
}

/// Uma ação de `FERRO_ACOES` ("clique 640 245", "digita texto", "tecla Enter",
/// "espera 2000", "foco seletor", "js código", "mouse x y"). Devolve a navegação pedida.
fn act(session: &mut ferro::script::Session, a: &Args, action: &str) -> Option<ferro::script::Nav> {
    let (verb, rest) = action.split_once(' ').unwrap_or((action, ""));
    log(format!("ação: {action}"));
    let nav = match verb {
        "clique" => {
            let n: Vec<f32> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            if n.len() == 2 {
                let _ = session.render();
                session.click(n[0], n[1])
            } else {
                None
            }
        }
        "mouse" => {
            let n: Vec<f32> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            if n.len() == 2 {
                let _ = session.render();
                let t = std::time::Instant::now();
                let c = session.mouse_move(Some((n[0], n[1])));
                let _ = session.render();
                log(format!("mouse: ponteiro {c:?}, eventos + novo layout em {} ms", t.elapsed().as_millis()));
            }
            None
        }
        "digita" => {
            for c in rest.chars() {
                let t = c.to_string();
                session.key(&t, Some(&t), false, false, false, false);
            }
            None
        }
        "tecla" => {
            session.key(rest.trim(), None, false, false, false, false);
            None
        }
        "foco" => {
            let code = format!("document.querySelector({:?}).focus()", rest.trim());
            if let Err(e) = session.js.eval(&code, "acao") {
                log(session.js.error_text(&e));
            }
            None
        }
        "js" => {
            match session.js.eval(rest, "acao") {
                Ok(v) => log(format!("  = {}", session.js.inspect(&v))),
                Err(e) => log(session.js.error_text(&e)),
            }
            None
        }
        "espera" => {
            let ms: u64 = rest.trim().parse().unwrap_or(1000);
            settle(session, a, std::time::Duration::from_millis(ms), false);
            None
        }
        _ => {
            log(format!("ação desconhecida: {verb}"));
            None
        }
    };
    nav.or_else(|| {
        settle(session, a, std::time::Duration::from_millis(1500), false);
        session.take_navigation()
    })
}

fn render_png_inner(a: &Args) -> Result<(), String> {
    let start = Instant::now();
    ferro::html::set_scripting(a.js);
    let target = a.target.as_deref().ok_or("informe uma URL ou um arquivo HTML")?;
    let mut go = ferro::script::Nav {
        url: resolve_target(target)?,
        post: None,
    };
    let net = Net::new();
    let fonts = std::rc::Rc::new(Fonts::system()?);
    // FERRO_ACOES="clique 640 245; digita texto; tecla Enter": interage com a página
    let mut actions: std::collections::VecDeque<String> = std::env::var("FERRO_ACOES")
        .unwrap_or_default()
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut hops = 0;
    let mut session = loop {
        let mut session = open_session(a, &net, &fonts, &go)?;
        let mut nav = session.take_navigation();
        while nav.is_none() {
            let Some(action) = actions.pop_front() else { break };
            nav = act(&mut session, a, &action);
        }
        match nav {
            Some(next) if hops < 8 => {
                log(format!("a página pediu para navegar para {}", next.url));
                hops += 1;
                go = next;
            }
            _ => break session,
        }
    };
    // depuração: FERRO_AVALIA="código" roda na página e mostra o resultado
    if let Ok(code) = std::env::var("FERRO_AVALIA") {
        match session.js.eval(&code, "avalia") {
            Ok(v) => println!("{}", session.js.inspect(&v)),
            Err(e) => println!("{}", session.js.error_text(&e)),
        }
    }
    if a.dump_dom {
        println!("{}", session.host.borrow().page.doc.dump());
    }

    let t = Instant::now();
    let page = session.render();
    log(format!(
        "estilo + layout: {} itens, página com {:.0} px, {:.0} ms",
        page.items.len(),
        page.height,
        ms(t)
    ));
    let frames = session.video_frames();
    let paused = session.paused_videos();
    let hb = session.host.borrow();
    let loaded = &hb.page;
    if a.dump_boxes {
        let media = ferro::css::MediaContext {
            width: a.width,
            height: a.height,
        };
        let mut sheets = vec![(
            std::rc::Rc::new(ferro::css::parse_stylesheet(ferro::UA_STYLESHEET, &media)),
            ferro::style::Origin::UserAgent,
        )];
        for s in &loaded.css {
            sheets.push((std::rc::Rc::new(ferro::css::parse_stylesheet(&s.text, &media)), ferro::style::Origin::Author));
        }
        let styles = ferro::style::compute(&loaded.doc, &sheets, &media);
        if let Some(root) = layout::build_box_tree(&loaded.doc, &styles) {
            println!("{}", root.dump(&loaded.doc));
        }
    }

    if let Some(out) = &a.out {
        let t = Instant::now();
        let h = page.height.min(a.max_h).ceil() as usize;
        let mut canvas = Canvas::new(a.width as usize, h, page.background);
        canvas.videos = frames;
        canvas.paused = paused;
        if std::env::var_os("FERRO_MIDIA_LOG").is_some() {
            let vids: Vec<_> = page
                .items
                .iter()
                .filter_map(|it| match it {
                    ferro::paint::DisplayItem::Video { rect, node } => Some((*node, *rect)),
                    _ => None,
                })
                .collect();
            log(format!("vídeos na pintura: {vids:?}; quadros: {:?}", canvas.videos.keys().collect::<Vec<_>>()));
        }
        canvas.paint(&page.items, &fonts, 0.0);
        canvas.save_png(out)?;
        log(format!("pintura: {:.0} ms", ms(t)));
        log(format!("título: {}", loaded.current_title()));
        log(format!("pronto: {} ({}×{}) em {:.0} ms", out.display(), a.width, h, ms(start)));
    }
    Ok(())
}
