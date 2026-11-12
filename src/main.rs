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

