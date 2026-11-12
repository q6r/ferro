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
