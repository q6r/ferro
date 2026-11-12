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

