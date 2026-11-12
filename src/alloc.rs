//! Alocador de memória do Ferro: rápido para os objetos pequenos, que são a
//! maioria (nós da AST, objetos JS, strings curtas, nós do DOM).
//!
//! - Tamanhos até 1 KiB caem em "classes" (16, 32, 48... bytes). Cada thread
//!   tem uma lista livre por classe e um bloco de 64 KiB de onde corta novos
//!   pedaços: alocar e liberar são poucas instruções, sem trava.
//! - O que é maior vai direto para o alocador do sistema.
//! - Quando uma thread termina, as listas e o resto do bloco dela vão para um
//!   estoque global, que as outras threads reaproveitam (a rede usa threads
//!   de vida curta).
//!
//! A memória dos objetos pequenos não volta para o sistema operacional, só é
//! reaproveitada.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};

/// Maior tamanho atendido pelas classes.
const SMALL: usize = 1024;
/// Classes: de 16 em 16 até 256, depois de 64 em 64 até 1024.
const NCLASS: usize = 16 + 12;
const CHUNK: usize = 64 * 1024;

#[inline]
fn class_of(size: usize) -> usize {
    if size <= 256 {
        (size.max(1) + 15) / 16 - 1
    } else {
        16 + (size - 256 + 63) / 64 - 1
    }
}

#[inline]
fn class_size(c: usize) -> usize {
    if c < 16 { (c + 1) * 16 } else { 256 + (c - 15) * 64 }
}

struct Local {
    free: [*mut u8; NCLASS],
    bump: *mut u8,
    end: *mut u8,
}

impl Local {
    const fn new() -> Local {
        Local {
            free: [null_mut(); NCLASS],
            bump: null_mut(),
            end: null_mut(),
        }
    }
}

thread_local! {
    // sem destrutor: pode ser usado até o último instante da thread
    static LOCAL: UnsafeCell<Local> = const { UnsafeCell::new(Local::new()) };
    static GUARD: ExitGuard = const { ExitGuard };
    /// 0 = guarda não registrada, 1 = registrando (reentrada), 2 = pronta.
    static REGISTERED: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

/// Registra a devolução da memória ao fim da thread (registrar pode alocar:
/// as alocações de dentro do registro passam direto).
#[cold]
fn ensure_guard() {
    let _ = REGISTERED.try_with(|r| {
        if r.get() == 0 {
            r.set(1);
            let _ = GUARD.try_with(|_| ());
            r.set(2);
        }
    });
}

/// Estoque global: listas devolvidas por threads que terminaram.
struct Shared {
    free: [*mut u8; NCLASS],
    /// Restos de blocos (início, fim).
    spare: [(*mut u8, *mut u8); 64],
    nspare: usize,
}

struct SpinShared {
    lock: AtomicBool,
    data: UnsafeCell<Shared>,
}

unsafe impl Sync for SpinShared {}

static SHARED: SpinShared = SpinShared {
    lock: AtomicBool::new(false),
    data: UnsafeCell::new(Shared {
        free: [null_mut(); NCLASS],
        spare: [(null_mut(), null_mut()); 64],
        nspare: 0,
    }),
};

fn with_shared<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    while SHARED
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
    let r = f(unsafe { &mut *SHARED.data.get() });
    SHARED.lock.store(false, Ordering::Release);
    r
}

/// Ao fim da thread, devolve o que ela tinha ao estoque global.
struct ExitGuard;

impl Drop for ExitGuard {
