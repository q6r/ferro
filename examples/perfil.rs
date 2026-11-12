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

