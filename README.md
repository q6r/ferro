# Ferro

Navegador com motor próprio, escrito do zero e 100% em Rust (sem nenhuma dependência em C/C++, inclusive a criptografia do HTTPS). Meta de longo prazo: abrir YouTube e Gmail.

## Uso

```
cargo run --release                                  # abre o navegador
cargo run --release -- https://pt.wikipedia.org      # abre já numa página
cargo run --release -- https://example.com -o p.png  # renderiza num PNG, sem janela (roda os scripts)
cargo run --release -- --captura ui.png --tema moca  # print da interface, sem janela
cargo run --release -- --js script.js                # executa JavaScript no motor próprio
cargo run --release -- --js                          # console interativo de JavaScript
cargo run --release -- --help
```

Atalhos: Ctrl+K busca, Ctrl+L / F6 / Alt+D edita o endereço, Ctrl+T nova aba, Ctrl+W fecha, Ctrl+B mostra/esconde a lista
de abas, Ctrl+Tab e Ctrl+1..9 trocam de aba, F5 / Ctrl+R recarrega, Alt+← / Alt+→ histórico, Ctrl + / − / 0 zoom,
Espaço / PgDn / setas / Home / End rolam. Ctrl+clique ou clique do meio abre o link em outra aba.
Na nova aba, é só começar a digitar. Ctrl+F busca na página (Enter / Shift+Enter / F3 andam pelas ocorrências),
arrastar o mouse seleciona texto, Ctrl+A / Ctrl+C selecionam e copiam, F11 tela cheia (Esc sai). Arquivos que não
são página vão para a pasta Downloads.

Cookies e localStorage ficam em `%APPDATA%\Ferro` (`cookies.txt`, `armazenamento.txt`): o login continua depois de
fechar. No modo PNG, `FERRO_DADOS=pasta` usa um perfil.

Captura da interface sem janela: `--captura x.png --tema <carvao|moca|crepusculo|nevoa|claro> --layout <lateral|capsulas>
--digitar "texto" --abas url1,url2 --buscar "texto" --selecionar-tudo`.

## Arquitetura

| Parte | Arquivo | Observação |
|---|---|---|
| Rede (http, file:, data:, charset) | `src/net.rs` | HTTP via `ureq`, TLS via `rustls` + RustCrypto (Rust puro) |
| HTML → DOM | `src/html.rs`, `src/dom.rs` | próprio |
| CSS, seletores, media queries | `src/css.rs` | próprio |
| Cascata, herança, `var()` | `src/style.rs`, `src/ua.css` | próprio |
| Layout de bloco e de linha, imagens | `src/layout.rs` | próprio |
| SVG | `src/svg.rs` | próprio (ícones e `<img>` SVG) |
| Pintura em software | `src/paint.rs` | próprio |
| Pipeline e thread de renderização por aba | `src/engine.rs` | próprio |
| Interface: abas, omnibox, nova aba, configurações | `src/ui/` | própria; janela via `winit` + `softbuffer` |
| Motor JavaScript (Ferro JS) | `js/` (crate `ferro-js`) | próprio, sem dependências |
| Scripts das páginas (DOM, eventos, rede, armazenamento) | `src/script/` | próprio; parte em JS (`prelude.js`) |
| Cookies | `src/cookies.rs` | próprio, compartilhado entre as abas |
| Perfil em disco (cookies, localStorage) | `src/dados.rs` | próprio |
| Canvas 2D (rasterizador) | `src/canvas.rs` | próprio |
| Busca na página, seleção, avisos | `src/ui/pagetools.rs` | próprio |

## Ferro JS

Motor de JavaScript próprio, em `js/`: lexer → parser (AST) → análise de capturas → compilador para bytecode → máquina virtual.

| Parte | Arquivo |
|---|---|
| Lexer e parser (ES2023) | `js/src/lexer.rs`, `js/src/parser.rs`, `js/src/ast.rs` |
| Análise de capturas (o que vai para o heap) | `js/src/resolve.rs` |
| Compilador para bytecode | `js/src/compiler.rs` |
| Máquina virtual, operações do spec | `js/src/vm.rs`, `js/src/ops.rs` |
| Strings UTF-16 com cordas | `js/src/string.rs` |
| Expressões regulares (parser, bytecode, backtracking) | `js/src/regex.rs` |
| BigInt (precisão arbitrária) | `js/src/bigint.rs` |
| Idioma e fuso do sistema para o motor | `src/jshost.rs` |
| Objetos embutidos (Object, Array, JSON, Map, Date, console...) | `js/src/builtins/` |
| Módulos ES (cada módulo vira uma função assíncrona) | `js/src/module.rs` |
| Coletor de ciclos (trial deletion sobre os `Rc`) | `js/src/gc.rs` |

Testes: `cargo test -p ferro-js` e o teste diferencial contra o Node (`python js/testes/diferencial.py`), que roda os
scripts de `js/testes/` nos dois motores e compara a saída linha a linha.

Suporta ES2023 quase completo: classes com `#privados`, desestruturação, geradores, Promise, `async`/`await`, geradores async,
`for await`, timers, fila de microtarefas, expressões regulares (motor próprio, com lookbehind e `\p{...}`), BigInt,
ArrayBuffer/typed arrays/DataView, Proxy/Reflect, Date no fuso local e Intl (en, pt, es, fr, de, it), módulos ES
(`import`/`export`, `import()`, `import.meta`, `await` no topo).

Memória: os objetos são contados por referência e um coletor de ciclos (`gc.rs`) acha os ciclos sem dono — sem
precisar conhecer as raízes. WeakMap/WeakSet/WeakRef não seguram as chaves (efêmeros). `FERRO_SEM_GC=1` desliga,
`FERRO_GC_DEBUG=1` mostra o que está vivo. `FERRO_BYTECODE=1` mostra o bytecode (com as superinstruções do otimizador
de janela; `FERRO_SEM_PEEPHOLE=1` desliga o otimizador).

## Visual

- Dois layouts: **Lateral** (trilho de ícones + lista de abas, kit 4) e **Cápsulas** (abas em pílulas no topo).
- A página fica num painel flutuante; o endereço é uma cápsula; a busca é uma paleta de vidro fosco (kit "Search or Ask").
- UI kits e paletas: `assets/uikits`, `assets/pallets`.
- Temas: Carvão, Moca, Crepúsculo, Névoa e Claro (`src/ui/theme.rs`).
- Ícones: [Mage Icons](https://mageicons.com), licença Apache 2.0 (`assets/icons/mage/LICENSE.txt`).

