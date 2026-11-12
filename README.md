# Ferro

Navegador com motor próprio, escrito do zero e 100% em Rust — sem nenhuma dependência em C/C++, inclusive a
criptografia do HTTPS e os decodificadores de vídeo. HTML, CSS, layout, pintura, o motor JavaScript, a interface:
tudo feito aqui. Meta: abrir YouTube e Gmail.

![Busca no YouTube aberta no Ferro (tema Carvão)](docs/previews/youtube.png)

## Prévias

| | |
|:---:|:---:|
| ![Wikipédia](docs/previews/wikipedia.png) | ![Buscar na página](docs/previews/busca-na-pagina.png) |
| Wikipédia: grid, float, tabelas, fontes web (tema Claro) | Ctrl+F com as ocorrências destacadas (tema Moca) |
| ![Nova aba](docs/previews/nova-aba.png) | ![Paleta de busca](docs/previews/paleta.png) |
| Nova aba com relógio e atalhos (tema Crepúsculo) | Paleta de busca de vidro fosco (tema Névoa) |
| ![Hacker News](docs/previews/hacker-news.png) | ![Login do Google](docs/previews/gmail.png) |
| Layout "Cápsulas": abas no topo | Login do Gmail rodando o JavaScript do Google |
| ![Canvas 2D](docs/previews/canvas.png) | ![Transformações](docs/previews/transform.png) |
| Canvas 2D com rasterizador próprio | `transform`, `box-shadow`, `text-shadow` |
| ![Escritas e emoji](docs/previews/escritas.png) | ![Vídeo](exemplos/video.png) |
| Chinês, japonês, coreano, devanágari e emoji colorido | `<video>` com MP4 (H.264/AV1 + AAC) decodificado em Rust |

## O que já funciona

- **Páginas de verdade**: YouTube (busca, miniaturas, navegação interna e vídeo tocando com som), login do Google,
  Wikipédia, Hacker News, g1, sites com módulos ES (vite.dev), players embutidos em `<iframe>`.
- **Motor JavaScript próprio** (Ferro JS): ES2023 quase completo, módulos ES, Promise/async, RegExp, BigInt, Proxy,
  typed arrays, Intl, coletor de ciclos — saída idêntica à do Node 24 no teste diferencial.
- **CSS moderno**: flexbox, grid, float, tabelas, `position` (inclusive `fixed` de verdade), transformações, sombras,
  `:hover`/`:focus`, variáveis, `@font-face`, `::before`/`::after`, rolagem dentro de elementos.
- **Plataforma web**: DOM, eventos de mouse e teclado, `fetch`/XHR, cookies, localStorage, Custom Elements,
  Shadow DOM, Canvas 2D, MSE, tela cheia, `postMessage` entre quadros.
- **Navegador**: abas, histórico, busca na página, seleção e cópia de texto, menu de contexto, downloads,
  `<select>` com lista, tela cheia, temas, zoom, cache HTTP e perfil (login continua depois de fechar).

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
| Busca na página, seleção, menu de contexto, lista do `<select>` | `src/ui/pagetools.rs` | próprio |
| `<iframe>` (cada quadro numa sessão própria) | `src/script/frames.rs` | próprio |
| Cache HTTP (memória + disco) | `src/httpcache.rs` | próprio |

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

## Scripts nas páginas (M4)

Cada página vira uma sessão (`src/script/`): o Ferro JS com o DOM da página por baixo.

- `dom.rs`: Node, Element, Document, Text, DocumentFragment, ShadowRoot em Rust, direto sobre a arena do DOM
  (innerHTML/outerHTML, seletores com `:scope`, `matches`/`closest`, medidas do layout, elementos personalizados,
  shadow DOM com slots, `document.write`, documentos extras para `createHTMLDocument`/`DOMParser`).
- `web.rs`: rede assíncrona (`fetch`/XHR, com cookies e redirecionamentos), URL, localStorage/sessionStorage,
  base64, TextEncoder/Decoder, `getComputedStyle`, `matchMedia`, `requestAnimationFrame`.
- `prelude.js`: eventos (captura/alvo/bolha, `on*`), `style`, `classList`, `dataset`, atributos refletidos,
  formulários, Headers/Request/Response, Blob/FormData, AbortController, MutationObserver, IntersectionObserver,
  MessageChannel, location/history/navigator, teclado (digitação em campos, Enter envia o formulário, Tab).
- `<script>` clássicos rodam em ordem, aos poucos no laço da aba (cada um assim que chega pela rede; a página
  aparece e responde nesse meio-tempo), depois os módulos (`type=module`, com import maps), depois
  `DOMContentLoaded`; o `load` sai quando os recursos chegam. Scripts inseridos depois também rodam.
- O laço da aba roda timers, rede e animações; o layout é refeito quando o DOM muda. Cliques e teclas vão para a
  página, que decide (`preventDefault`); links e formulários viram navegação.
- Mouse: `:hover`/`:active`/`:focus`/`:focus-visible`/`:focus-within`, mouseover/out/enter/leave/move e pointer*,
  ponteiro do CSS (`cursor`), `pointer-events`. O alvo é o que está pintado por cima (camadas e z-index).
- Rolagem dentro de elementos (`overflow: auto/scroll`) pela roda do mouse e por `scrollTop`/`scrollTo`/
  `scrollIntoView`; tela cheia (`requestFullscreen`, `:fullscreen`).
- `<canvas>` 2D completo (caminhos, traços, gradientes, padrões, imagens, texto, recorte, composição, pixels,
  `toDataURL`, `OffscreenCanvas`, `createImageBitmap`): desenha em `src/canvas.rs`; o quadro novo vai para a tela
  sem refazer o layout. Exemplo: `exemplos/canvas.html`.

Depuração sem janela: `FERRO_AVALIA="js"` avalia na página carregada; `FERRO_VIGIA="expr"` mostra quando o valor muda
de um script para outro; `FERRO_ACOES="foco #campo; digita texto; tecla Enter; clique 640 245; espera 2000"`
interage com a página (e segue as navegações; `mouse x y` move o mouse e mede o restilo); `FERRO_TEMPOS_JS=1` mostra
análise/compilação/execução de cada script; `FERRO_TEMPOS=1` mostra CSS/cascata/caixas/layout, o índice de regras e o
coletor; `--sem-js` desliga os scripts; `--espera <ms>` limita a espera.

Perfilador por amostragem (Windows): `cargo run --profile perfil --example perfil -- <arquivo.js | url> [--render]`
(`--render` refaz estilo e layout 30 vezes; `FERRO_PERFIL_FOCO=função` filtra as amostras que passam por ela;
`FERRO_PERFIL_N=40` mostra mais linhas).

Já funcionam: YouTube (página inicial, busca com miniaturas, navegação interna do app), tela de login do Google/Gmail
(envio do e-mail por POST e resposta do servidor), g1, Wikipédia, sites com módulos ES (vite.dev), vídeo do YouTube.

## Vídeo e áudio (`src/media/`)

- Media Source Extensions (`MediaSource`, `SourceBuffer`, `TimeRanges`) no prelúdio, com o estado em Rust: o player
  do site (YouTube) baixa os segmentos e o Ferro os lê (`mp4.rs`: MP4 fragmentado, incremental).
- Decodificadores em Rust puro, sem assembly nem C (`decode.rs`): H.264 Baseline/Main (`rusty_h264`), AV1 8 bits
  (`rav1d`) e AAC (`symphonia`). Formatos aceitos: `video/mp4` com `avc1.42*`/`avc1.4d*`/`av01.0.*` e `audio/mp4` AAC.
- Arquivos MP4 comuns (`<video src="x.mp4">` ou `<source>`): o arquivo é baixado inteiro e lido pelas tabelas do
  `stbl`; `<video controls>` toca/pausa com um clique e, parado, mostra o botão de play. Exemplo: `exemplos/video.html`.
- Reprodução (`player.rs`): threads de decodificação alimentadas ~3 s à frente do relógio; uma thread apresentadora
  escolhe o quadro do momento e manda direto para a interface (sem passar pelo laço do JS); áudio pelo `cpal`, com
  reamostragem linear para a taxa da placa de som.
- O `<video>` vira um item próprio na lista de pintura, trocado pelo quadro atual sem refazer o layout (os controles
  do player ficam por cima, na ordem de empilhamento certa).

Depuração: `FERRO_MIDIA_LOG=1` (tipos pedidos, decodificação, relógio), `FERRO_MIDIA_DUMP=pasta` (grava o que chega por
`appendBuffer`); `cargo run --release --example midia -- arquivo.mp4 pasta-png` testa os decodificadores.

## Desempenho

- Alocador próprio (`src/alloc.rs`): classes de tamanho por thread; tirou ~25% do tempo gasto no heap do Windows.
- Hash rápido (FxHash) nas tabelas do motor; strings JS guardam o próprio hash.
- Objetos pequenos sem tabela hash (busca linear até 8 propriedades); cache por instrução para globais.
- Compilação do app do YouTube (10,5 MB de JS): de ~5,8 s para ~1,3 s; scripts iniciais do YouTube: de ~10 s para ~2,4 s.
- Folhas de estilo analisadas uma vez só (os redesenhos provocados por scripts não reanalisam o CSS).
- Cascata: índice das regras guardado entre os cálculos, filtro de Bloom dos ancestrais, regras indexadas também por
  atributo, variáveis CSS encadeadas (sem cópia por elemento). YouTube: ~200 ms → ~40 ms por restilo.
- Interpretador: superinstruções (otimizador de janela) e argumentos direto para os registradores.
