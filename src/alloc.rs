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
    fn drop(&mut self) {
        let _ = LOCAL.try_with(|l| unsafe {
            let l = &mut *l.get();
            with_shared(|s| {
                for c in 0..NCLASS {
                    let head = l.free[c];
                    if head.is_null() {
                        continue;
                    }
                    // emenda a lista da thread no começo da global
                    let mut tail = head;
                    while !(*(tail as *mut *mut u8)).is_null() {
                        tail = *(tail as *mut *mut u8);
                    }
                    *(tail as *mut *mut u8) = s.free[c];
                    s.free[c] = head;
                    l.free[c] = null_mut();
                }
                if !l.bump.is_null() && l.end as usize - l.bump as usize >= 256 && s.nspare < s.spare.len() {
                    s.spare[s.nspare] = (l.bump, l.end);
                    s.nspare += 1;
                }
                l.bump = null_mut();
                l.end = null_mut();
            });
        });
    }
}

pub struct Alloc;

impl Alloc {
    #[cold]
    unsafe fn refill(l: &mut Local, c: usize, size: usize) -> *mut u8 {
        // primeiro, o estoque global
        let got = with_shared(|s| {
            let head = s.free[c];
            if !head.is_null() {
                s.free[c] = null_mut();
                return (head, None);
            }
            if s.nspare > 0 {
                s.nspare -= 1;
                return (null_mut(), Some(s.spare[s.nspare]));
            }
            (null_mut(), None)
        });
        match got {
            (head, _) if !head.is_null() => {
                l.free[c] = unsafe { *(head as *mut *mut u8) };
                return head;
            }
            (_, Some((b, e))) => {
                l.bump = b;
                l.end = e;
            }
            _ => {}
        }
        if (l.end as usize).saturating_sub(l.bump as usize) < size {
            let chunk = unsafe { System.alloc(Layout::from_size_align_unchecked(CHUNK, 16)) };
            if chunk.is_null() {
                return null_mut();
            }
            l.bump = chunk;
            l.end = unsafe { chunk.add(CHUNK) };
        }
        let p = l.bump;
        l.bump = unsafe { p.add(size) };
        p
    }
}

unsafe impl GlobalAlloc for Alloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        if size > SMALL || layout.align() > 16 {
            return unsafe { System.alloc(layout) };
        }
        let c = class_of(size);
        let cs = class_size(c);
        let fast = LOCAL.try_with(|l| unsafe {
            let l = &mut *l.get();
            let head = l.free[c];
            if !head.is_null() {
                l.free[c] = *(head as *mut *mut u8);
                return head;
            }
            if !l.bump.is_null() && (l.end as usize) - (l.bump as usize) >= cs {
                let p = l.bump;
                l.bump = p.add(cs);
                return p;
            }
            null_mut()
        });
        let r = match fast {
            Ok(p) if !p.is_null() => return p,
            Ok(_) => {
                ensure_guard();
                LOCAL.try_with(|l| unsafe { Self::refill(&mut *l.get(), c, cs) })
            }
            Err(e) => Err(e),
        };
        match r {
            Ok(p) => p,
            // thread já sendo destruída: usa o sistema com o tamanho da classe
            Err(_) => unsafe { System.alloc(Layout::from_size_align_unchecked(class_size(c), 16)) },
        }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size();
        if size > SMALL || layout.align() > 16 {
            unsafe { System.dealloc(ptr, layout) };
            return;
        }
        let c = class_of(size);
        let ok = LOCAL.try_with(|l| unsafe {
            let l = &mut *l.get();
            *(ptr as *mut *mut u8) = l.free[c];
            l.free[c] = ptr;
        });
        if ok.is_err() {
            // sem lista local: vai para o estoque global
            with_shared(|s| unsafe {
                *(ptr as *mut *mut u8) = s.free[c];
                s.free[c] = ptr;
            });
        }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old = layout.size();
        let small_old = old <= SMALL && layout.align() <= 16;
        let small_new = new_size <= SMALL && layout.align() <= 16;
        if small_old && small_new && class_of(old) == class_of(new_size) {
            return ptr;
        }
        if !small_old && !small_new {
            return unsafe { System.realloc(ptr, layout, new_size) };
        }
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        let p = unsafe { self.alloc(new_layout) };
        if !p.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, p, old.min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        for s in 1..=SMALL {
            let c = class_of(s);
            assert!(c < NCLASS, "{s}");
            assert!(class_size(c) >= s, "{s}");
            if c > 0 {
                assert!(class_size(c - 1) < s, "{s}");
            }
        }
    }

    #[test]
    fn aloca_e_libera() {
        let a = Alloc;
        unsafe {
            let mut ps = Vec::new();
            for i in 1..2000usize {
                let l = Layout::from_size_align(i % 1100 + 1, 8).unwrap();
                let p = a.alloc(l);
                assert!(!p.is_null());
                std::ptr::write_bytes(p, (i % 251) as u8, l.size());
                ps.push((p, l, (i % 251) as u8));
            }
            for (p, l, v) in &ps {
                assert_eq!(*p.add(l.size() - 1), *v);
                a.dealloc(*p, *l);
            }
        }
        // threads que terminam devolvem a memória
        let h = std::thread::spawn(|| (0..1000).map(|i| vec![i as u8; 40]).collect::<Vec<_>>());
        assert_eq!(h.join().unwrap().len(), 1000);
    }
}
