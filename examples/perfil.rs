//! Perfilador por amostragem (ferramenta de desenvolvimento, só Windows x64).
//!
//! `cargo run --profile perfil --example perfil -- arquivo.js` roda o script no
//! Ferro JS enquanto outra thread suspende a do motor a cada ~1 ms, lê a pilha
//! e, no fim, mostra as funções onde o tempo foi gasto (própria e inclusiva).

#[global_allocator]
static ALLOC: ferro::alloc::Alloc = ferro::alloc::Alloc;

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() {
    eprintln!("o perfilador só funciona no Windows x64");
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn main() {
    use std::collections::{HashMap, HashSet};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Diagnostics::Debug::*;
    use windows_sys::Win32::System::SystemInformation::IMAGE_FILE_MACHINE_AMD64;
    use windows_sys::Win32::System::Threading::*;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first().cloned() else {
        eprintln!("uso: perfil arquivo.js [--so-compilar]");
        return;
    };
    let only_compile = args.iter().any(|a| a == "--so-compilar");
    let render_only = args.iter().any(|a| a == "--render");
    let src = if path.starts_with("http") { String::new() } else { std::fs::read_to_string(&path).expect("não li o arquivo") };

    let (tx, rx) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let marker = 0u8;
            let top = &marker as *const u8 as u64;
            tx.send((unsafe { GetCurrentThreadId() }, top)).unwrap();
            // dá tempo da thread de amostragem começar
            std::thread::sleep(Duration::from_millis(50));
            let t = Instant::now();
            if path.starts_with("http") || path.ends_with(".html") {
                // página inteira: carrega, roda os scripts e o laço de eventos por um tempo
                let url = if path.starts_with("http") {
                    url::Url::parse(&path).unwrap()
                } else {
                    url::Url::from_file_path(std::path::absolute(&path).unwrap()).unwrap()
                };
                let net = ferro::net::Net::new();
                let loaded = ferro::engine::load(&net, &url).expect("falha ao carregar");
                let fonts = std::rc::Rc::new(ferro::fonts::Fonts::system().unwrap());
                let mut s = ferro::script::Session::new(loaded, net, fonts, 1280.0, 800.0);
                let _ = s.render();
                s.run_scripts(&|| false);
                let t = Instant::now();
                let mut fired = false;
                while t.elapsed() < Duration::from_secs(if render_only { 4 } else { 8 }) {
                    s.tick();
                    if s.wants_frame() {
                        let _ = s.render();
                    }
                    if s.idle() && !fired {
                        s.finish_load();
                        fired = true;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                // --render: refaz estilo e layout várias vezes (perfil da cascata)
                if render_only {
                    for _ in 0..30 {
                        s.resize(1280.0, 800.0);
                        let _ = s.render();
                    }
                }
            } else if only_compile {
                let prog = ferro_js::parser::parse_program(&src).expect("erro de sintaxe");
                let _ = ferro_js::compiler::compile_script(&prog, "perfil").expect("erro ao compilar");
            } else {
                let mut js = ferro_js::Engine::new();
                js.vm.max_depth = 8000;
                js.set_output(|_, _| {});
                if let Err(e) = js.eval(&src, "perfil") {
                    eprintln!("{}", js.error_text(&e));
                }
                js.run_event_loop();
            }
            t.elapsed()
        })
        .unwrap();
    let (tid, stack_top) = rx.recv().unwrap();
    let h: HANDLE = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, tid) };
    let process = unsafe { GetCurrentProcess() };
    unsafe {
        SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS);
        // procura o .pdb ao lado do executável
        let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned())).unwrap_or_default();
        let path = std::ffi::CString::new(dir).unwrap();
        SymInitialize(process, path.as_ptr() as *const u8, 1);
    }

    // cópia da pilha da thread suspensa (o desenrolar acontece depois, sem
    // a thread parada: ela pode estar segurando o lock do heap)
    const SNAP: usize = 512 * 1024;
    let mut snaps: Vec<(CONTEXT, u64, Vec<u8>)> = Vec::new();
    let mut spare: Vec<Vec<u8>> = Vec::new();
    while !worker.is_finished() {
        let mut buf = spare.pop().unwrap_or_else(|| vec![0u8; SNAP]);
        unsafe {
            if SuspendThread(h) == u32::MAX {
                break;
            }
            let mut ctx: CONTEXT = std::mem::zeroed();
            ctx.ContextFlags = CONTEXT_FULL_AMD64;
            let ok = GetThreadContext(h, &mut ctx) != 0;
            let rsp = ctx.Rsp;
            let len = if ok && stack_top > rsp { ((stack_top - rsp) as usize + 64).min(SNAP) } else { 0 };
            if len > 0 {
                std::ptr::copy_nonoverlapping(rsp as *const u8, buf.as_mut_ptr(), len);
            }
            ResumeThread(h);
            if ok {
                buf.truncate(len.max(1));
                snaps.push((ctx, rsp, buf));
            } else {
                spare.push(buf);
            }
        }
        if snaps.len() > 20000 {
            break;
        }
        std::thread::sleep(Duration::from_micros(700));
    }
    // desenrola cada pilha lendo da cópia
    static mut SNAP_BASE: u64 = 0;
    static mut SNAP_PTR: *const u8 = std::ptr::null();
