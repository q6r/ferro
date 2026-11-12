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
    static mut SNAP_LEN: usize = 0;
    unsafe extern "system" fn read_mem(_p: HANDLE, addr: u64, out: *mut core::ffi::c_void, n: u32, got: *mut u32) -> windows_sys::core::BOOL {
        unsafe {
            let (base, ptr, len) = (SNAP_BASE, SNAP_PTR, SNAP_LEN);
            let n = n as usize;
            if addr >= base && addr + n as u64 <= base + len as u64 {
                std::ptr::copy_nonoverlapping(ptr.add((addr - base) as usize), out as *mut u8, n);
            } else {
                // código e tabelas de desenrolar: lê com segurança (endereço pode ser inválido)
                let mut read = 0usize;
                let ok = ReadProcessMemory(GetCurrentProcess(), addr as *const core::ffi::c_void, out, n, &mut read);
                if !got.is_null() {
                    *got = read as u32;
                }
                return ok;
            }
            if !got.is_null() {
                *got = n as u32;
            }
            1
        }
    }
    let mut stacks: Vec<Vec<u64>> = Vec::new();
    for (mut ctx, rsp, buf) in snaps {
        unsafe {
            SNAP_BASE = rsp;
            SNAP_PTR = buf.as_ptr();
            SNAP_LEN = buf.len();
            let mut frame: STACKFRAME64 = std::mem::zeroed();
            frame.AddrPC.Offset = ctx.Rip;
            frame.AddrPC.Mode = AddrModeFlat;
            frame.AddrFrame.Offset = ctx.Rbp;
            frame.AddrFrame.Mode = AddrModeFlat;
            frame.AddrStack.Offset = ctx.Rsp;
            frame.AddrStack.Mode = AddrModeFlat;
            let mut stack = vec![ctx.Rip];
            for _ in 0..48 {
                let ok = StackWalk64(
                    IMAGE_FILE_MACHINE_AMD64 as u32,
                    process,
                    h,
                    &mut frame,
                    &mut ctx as *mut CONTEXT as *mut core::ffi::c_void,
                    Some(read_mem),
                    Some(SymFunctionTableAccess64),
                    Some(SymGetModuleBase64),
                    None,
                );
                if ok == 0 || frame.AddrPC.Offset == 0 {
                    break;
                }
                if frame.AddrStack.Offset < rsp || frame.AddrStack.Offset >= rsp + buf.len() as u64 {
                    break;
                }
                if stack.last() != Some(&frame.AddrPC.Offset) {
                    stack.push(frame.AddrPC.Offset);
                }
            }
            stacks.push(stack);
        }
    }
    let elapsed = worker.join().unwrap();

    // endereço → nome da função
    let mut names: HashMap<u64, String> = HashMap::new();
    let mut name_of = |addr: u64| -> String {
        names
            .entry(addr)
            .or_insert_with(|| unsafe {
                const MAX: usize = 512;
                let mut buf = vec![0u8; std::mem::size_of::<SYMBOL_INFO>() + MAX];
                let sym = buf.as_mut_ptr() as *mut SYMBOL_INFO;
                (*sym).SizeOfStruct = std::mem::size_of::<SYMBOL_INFO>() as u32;
                (*sym).MaxNameLen = MAX as u32;
                let mut disp = 0u64;
                if SymFromAddr(process, addr, &mut disp, sym) != 0 {
                    let len = (*sym).NameLen as usize;
                    let ptr = (*sym).Name.as_ptr() as *const u8;
                    String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned()
                } else {
                    format!("{addr:#x}")
                }
            })
            .clone()
    };
    let mut own: HashMap<String, usize> = HashMap::new();
    let mut incl: HashMap<String, usize> = HashMap::new();
    // FERRO_PERFIL_FOCO=nome: só as amostras que passam por essa função
    if let Ok(foco) = std::env::var("FERRO_PERFIL_FOCO") {
        stacks.retain(|st| st.iter().any(|&a| name_of(a).contains(foco.as_str())));
    }
    let lines: usize = std::env::var("FERRO_PERFIL_N").ok().and_then(|v| v.parse().ok()).unwrap_or(35);
    for st in &stacks {
        if let Some(&leaf) = st.first() {
            *own.entry(name_of(leaf)).or_default() += 1;
        }
        let mut seen = HashSet::new();
        for &a in st {
            let n = name_of(a);
            if seen.insert(n.clone()) {
                *incl.entry(n).or_default() += 1;
            }
        }
    }
    let total = stacks.len().max(1) as f64;
    println!("{} amostras em {:.0} ms", stacks.len(), elapsed.as_secs_f64() * 1000.0);
    let show = |title: &str, m: &HashMap<String, usize>, n: usize| {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        println!("\n== {title}");
        for (name, c) in v.into_iter().take(n) {
            let short: String = name.chars().take(110).collect();
            println!("{:6.1}%  {short}", *c as f64 * 100.0 / total);
        }
    };
    show("tempo próprio", &own, lines);
    show("tempo inclusivo", &incl, lines);
}
