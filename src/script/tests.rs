//! Testes da ligação JS ↔ DOM (sessões montadas a partir de HTML em memória).

use std::rc::Rc;

use url::Url;

use super::{STACK_SIZE, Session};
use crate::engine::Loaded;
use crate::fonts::Fonts;
use crate::net::Net;

/// Roda `f` numa thread com pilha grande (o interpretador é recursivo).
pub(super) fn with_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(STACK_SIZE).spawn(f).unwrap().join().unwrap();
}

pub(super) fn session(html: &str) -> Session {
    let url = Url::parse("https://teste.ferro/pagina").unwrap();
    let doc = crate::html::parse(html);
    let loaded = Loaded {
        url: url.clone(),
        base: url,
        status: 200,
        doc,
        css: Vec::new(),
        title: String::new(),
        sheet_texts: Default::default(),
    };
    let fonts = Rc::new(Fonts::system().expect("fontes do sistema"));
    let mut s = Session::new(loaded, Net::new(), fonts, 800.0, 600.0);
    s.disable_resources();
    // as <style> do HTML (o carregamento normal já as junta)
    s.host.borrow_mut().styles_dirty = true;
    let _ = s.render();
    s.run_scripts(&|| false);
    s
}

pub(super) fn eval(s: &mut Session, code: &str) -> String {
    match s.js.eval(code, "teste") {
        Ok(v) => match v {
            ferro_js::Value::String(t) => t.to_rust(),
            v => s.js.inspect(&v),
        },
        Err(e) => s.js.error_text(&e),
    }
}

#[test]
fn clique_link_e_eventos() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0">
            <a id=l href="https://exemplo.com/x" style="display:block;height:50px">ir</a>
            <div id=d style="height:50px" onclick="this.textContent='clicado'">d</div>
            <a id=p href="/y" style="display:block;height:50px"><span>não</span></a>
            <script>
              window.log = [];
              document.getElementById('p').addEventListener('click', e => { log.push('p'); e.preventDefault(); });
              document.addEventListener('mousedown', e => log.push('down:' + e.target.id));
            </script></body>"#,
        );
        // link comum: o clique vira navegação
        let nav = s.click(10.0, 20.0);
        assert_eq!(nav.map(|u| u.url.to_string()), Some("https://exemplo.com/x".into()));
        // div com onclick inline: muda o DOM, sem navegar
        assert!(s.click(10.0, 70.0).is_none());
        assert_eq!(eval(&mut s, "document.getElementById('d').textContent"), "clicado");
        // link com preventDefault: não navega (o alvo é o <span> dentro dele)
        assert!(s.click(5.0, 106.0).is_none());
        assert_eq!(eval(&mut s, "log.join()"), "down:l,down:d,down:,p");
        // o alvo do último clique foi o <span> (sem id)
        assert_eq!(eval(&mut s, "document.elementFromPoint(5, 106).localName"), "span");
    });
}

#[test]
fn location_e_formulario() {
    with_stack(|| {
        let mut s = session(
            r#"<form id=f action="/busca"><input name=q value="a b"><button id=b>ir</button></form>
            <script>location.hash = 'sec';</script>"#,
        );
        // só o fragmento: não recarrega
        assert!(s.take_navigation().is_none());
        assert_eq!(eval(&mut s, "location.href"), "https://teste.ferro/pagina#sec");
        eval(&mut s, "document.getElementById('b').click()");
        assert_eq!(eval(&mut s, "1"), "1");
        let nav = s.take_navigation().map(|u| u.url.to_string());
        assert_eq!(nav, Some("https://teste.ferro/busca?q=a+b".into()));
        eval(&mut s, "location.href = '/outra'");
        assert_eq!(s.take_navigation().map(|u| u.url.to_string()), Some("https://teste.ferro/outra".into()));
        eval(&mut s, "history.pushState({}, '', '/spa/1')");
        assert_eq!(eval(&mut s, "location.pathname"), "/spa/1");
        assert!(s.take_navigation().is_none());
    });
}

#[test]
fn timers_e_raf() {
    with_stack(|| {
        let mut s = session(
            r#"<div id=d></div><script>
              setTimeout(() => d.textContent += 't', 0);
              requestAnimationFrame(ts => d.textContent += (typeof ts === 'number' ? 'r' : '?'));
              queueMicrotask(() => d.textContent += 'm');
            </script>"#,
        );
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < 200 {
            s.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let txt = eval(&mut s, "document.getElementById('d').textContent");
        assert!(txt.starts_with('m') && txt.contains('t') && txt.contains('r'), "{txt}");
        // o DOM mudou: a aba precisa redesenhar
        assert!(s.wants_frame() || s.host.borrow().dirty);
    });
}

#[test]
fn document_write_e_scripts_em_ordem() {
    with_stack(|| {
        let mut s = session(
            r#"<body><script>window.ordem = ['a']; document.write('<p id=w>escrito</p><script>ordem.push(\'b\')<\/script>');</script>
            <script>ordem.push('c'); window.temW = !!document.getElementById('w');</script></body>"#,
        );
        assert_eq!(eval(&mut s, "ordem.join()"), "a,b,c");
        assert_eq!(eval(&mut s, "temW"), "true");
        assert_eq!(eval(&mut s, "document.getElementById('w').previousElementSibling.localName"), "script");
    });
}

#[test]
fn digitar_em_campos() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0"><form action="/busca"><input id=q name=q style="display:block;width:300px;height:30px"></form>
            <textarea id=t style="display:block;height:40px"></textarea>
            <script>
              window.eventos = [];
              q.addEventListener('keydown', e => eventos.push('down:' + e.key));
              q.addEventListener('input', e => eventos.push('input:' + e.data));
              q.addEventListener('change', () => eventos.push('change'));
            </script></body>"#,
        );
        assert!(!s.editing());
        // clicar no campo dá o foco
        assert!(s.click(10.0, 10.0).is_none());
        assert!(s.editing());
        for (k, t) in [("O", "O"), ("l", "l"), ("á", "á"), ("x", "x")] {
            s.key(k, Some(t), false, false, false, false);
        }
        s.key("Backspace", None, false, false, false, false);
        assert_eq!(eval(&mut s, "q.value"), "Olá");
        assert_eq!(eval(&mut s, "eventos.slice(0, 2).join()"), "down:O,input:O");
        // Enter envia o formulário
        s.key("Enter", None, false, false, false, false);
        assert_eq!(s.take_navigation().map(|u| u.url.to_string()), Some("https://teste.ferro/busca?q=Ol%C3%A1".into()));
        // Tab passa o foco para o textarea (e o input dispara change)
        s.key("Tab", None, false, false, false, false);
        assert_eq!(eval(&mut s, "document.activeElement.id"), "t");
        assert_eq!(eval(&mut s, "eventos.includes('change')"), "true");
        s.key("a", Some("a"), false, false, false, false);
        s.key("Enter", None, false, false, false, false);
        s.key("b", Some("b"), false, false, false, false);
        assert_eq!(eval(&mut s, "t.value === 'a' + String.fromCharCode(10) + 'b'"), "true");
    });
}

#[test]
fn rolagem_dentro_de_elemento() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0"><div id=caixa style="height:100px;overflow:auto">
              <div style="height:400px">conteúdo</div><p id=fim style="margin:0;height:20px">fim</p></div>
            <script>window.rolou = 0; caixa.addEventListener('scroll', () => rolou++);</script></body>"#,
        );
        assert_eq!(eval(&mut s, "caixa.scrollHeight + ',' + caixa.clientHeight + ',' + caixa.scrollTop"), "420,100,0");
        // pela roda do mouse (motor)
        let caixa = match s.js.eval("caixa", "t") {
            Ok(v) => super::node_of(&v).unwrap(),
            Err(_) => panic!("sem caixa"),
        };
        assert!(s.scroll_element(caixa, 0.0, 150.0));
        let _ = s.render();
        assert_eq!(eval(&mut s, "caixa.scrollTop"), "150");
        // o conteúdo subiu junto
        assert_eq!(eval(&mut s, "Math.round(fim.getBoundingClientRect().top)"), "250");
        // pelo JS, com limite
        eval(&mut s, "caixa.scrollTop = 9999");
        assert_eq!(eval(&mut s, "caixa.scrollTop"), "320");
        assert_eq!(eval(&mut s, "rolou"), "1");
    });
}

#[test]
fn mouse_hover_e_cursor() {
    with_stack(|| {
        let mut s = session(
            r#"<style>
                 #a:hover { width: 300px } #a { cursor: pointer; width: 200px; height: 50px }
                 #capa { position: absolute; top: 0; left: 0; width: 500px; height: 30px; pointer-events: none }
                 #menu { position: absolute; top: 100px; left: 0; width: 100px; height: 40px; z-index: 5 }
               </style>
               <body style="margin:0"><div id=a><span id=t>texto</span></div><div id=capa></div>
               <div style="height:200px"><p id=fundo style="margin:0;padding-top:100px;height:40px">fundo</p></div>
               <div id=menu></div>
               <script>
                 window.log = [];
                 for (const ty of ['mouseover', 'mouseout', 'mouseenter', 'mouseleave'])
                   a.addEventListener(ty, (e) => log.push(ty + ':' + e.target.id));
                 a.addEventListener('mousemove', (e) => log.push('move:' + e.clientX));
               </script></body>"#,
        );
        let _ = s.render();
        assert_eq!(eval(&mut s, "a.getBoundingClientRect().width"), "200");
        // a capa com pointer-events: none não pega o mouse
        let c = s.mouse_move(Some((10.0, 10.0)));
        assert_eq!(c, crate::style::Cursor::Pointer);
        assert_eq!(eval(&mut s, "log.join()"), "mouseover:t,mouseenter:a,move:10");
        assert_eq!(eval(&mut s, "a.matches(':hover') + ',' + t.matches(':hover')"), "true,true");
        let _ = s.render();
        assert_eq!(eval(&mut s, "a.getBoundingClientRect().width"), "300");
        // o menu (z-index) fica por cima do parágrafo, que é mais fundo na árvore
        eval(&mut s, "log = []");
        let c = s.mouse_move(Some((10.0, 110.0)));
        assert_eq!(c, crate::style::Cursor::Auto);
        assert_eq!(eval(&mut s, "log.join()"), "mouseout:t,mouseleave:a");
        assert_eq!(eval(&mut s, "document.querySelector(':hover:not(html):not(body)').id"), "menu");
        let _ = s.render();
        assert_eq!(eval(&mut s, "a.getBoundingClientRect().width"), "200");
    });
}

#[test]
fn modulos_es_na_pagina() {
    with_stack(|| {
        let mut s = session(
            r#"<script>window.log = []; document.addEventListener('DOMContentLoaded', () => log.push('dcl'));</script>
               <script type=importmap>{"imports": {"util": "data:text/javascript,export const z = 3;"}}</script>
               <script type=module>
                 import v, { x } from "data:text/javascript,export default 5; export const x = 7;";
                 import { z } from "util";
                 log.push('m1:' + (v + x + z));
                 const m = await import("data:text/javascript,export const y = 9;");
                 log.push('dyn:' + m.y + ':' + Object.prototype.toString.call(m));
               </script>
               <script>log.push('classic');</script>"#,
        );
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < 2000 && eval(&mut s, "log.length") != "4" {
            s.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(eval(&mut s, "log.join()"), "classic,m1:15,dcl,dyn:9:[object Module]");
        // import() de um script clássico; erro de especificador
        eval(&mut s, "import('data:text/javascript,export default 1').then(m => log.push('c:' + m.default)); import('nu').catch(e => log.push(e.name))");
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < 2000 && eval(&mut s, "log.length") != "6" {
            s.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(eval(&mut s, "log.slice(4).sort().join()"), "TypeError,c:1");
    });
}

#[test]
fn canvas_2d() {
    with_stack(|| {
        let mut s = session(
            r#"<canvas id=c width=40 height=20></canvas><script>
                 const x = c.getContext('2d');
                 x.fillStyle = 'red'; x.fillRect(0, 0, 10, 10);
                 x.save(); x.translate(20, 0); x.fillStyle = 'rgb(0, 0, 255)';
                 x.beginPath(); x.arc(5, 5, 4, 0, Math.PI * 2); x.fill(); x.restore();
                 x.strokeStyle = '#0f0'; x.lineWidth = 2; x.beginPath(); x.moveTo(0, 15); x.lineTo(40, 15); x.stroke();
                 const g = x.createLinearGradient(30, 0, 40, 0); g.addColorStop(0, 'black'); g.addColorStop(1, 'white');
                 x.fillStyle = g; x.fillRect(30, 0, 10, 10);
                 x.clearRect(0, 0, 2, 2);
               </script>"#,
        );
        let px = |s: &mut Session, x: i32, y: i32| eval(s, &format!("Array.from(x.getImageData({x}, {y}, 1, 1).data).join()"));
        assert_eq!(px(&mut s, 5, 5), "255,0,0,255");
        assert_eq!(px(&mut s, 0, 0), "0,0,0,0");
        assert_eq!(px(&mut s, 25, 5), "0,0,255,255");
        assert_eq!(px(&mut s, 20, 0), "0,0,0,0");
        assert_eq!(px(&mut s, 5, 14), "0,255,0,255");
        assert_eq!(eval(&mut s, "x.fillStyle = 'rgba(255,0,0,.5)'; x.fillStyle"), "rgba(255, 0, 0, 0.5)");
        assert_eq!(eval(&mut s, "x.getImageData(30, 5, 10, 1).data[0] < x.getImageData(30, 5, 10, 1).data[36]"), "true");
        assert_eq!(eval(&mut s, "x.measureText('abc').width > 10"), "true");
        assert_eq!(eval(&mut s, "c.toDataURL().startsWith('data:image/png;base64,iVBOR')"), "true");
        assert_eq!(eval(&mut s, "x.isPointInPath(25, 5)"), "false");
        assert_eq!(eval(&mut s, "x.beginPath(); x.rect(0, 0, 5, 5); x.isPointInPath(2, 2)"), "true");
        // a página mostra o canvas
        let frames = s.video_frames();
        assert_eq!(frames.len(), 1);
        // putImageData e texto
        eval(&mut s, "const d = x.createImageData(1, 1); d.data.set([1, 2, 3, 255]); x.putImageData(d, 39, 19);");
        assert_eq!(px(&mut s, 39, 19), "1,2,3,255");
        eval(&mut s, "x.fillStyle = '#000'; x.font = 'bold 16px sans-serif'; x.fillText('W', 0, 18)");
        assert_eq!(eval(&mut s, "Array.from(x.getImageData(0, 4, 14, 14).data).some((v, i) => i % 4 == 3 && v > 200)"), "true");
        // redimensionar apaga
        eval(&mut s, "c.width = 10");
        assert_eq!(eval(&mut s, "x.getImageData(5, 5, 1, 1).data[3] + ',' + c.width"), "0,10");
    });
}

#[test]
fn tela_cheia() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0"><div style="height:300px"></div><div id=v style="width:100px;height:50px"></div>
               <script>window.ev = 0; document.addEventListener('fullscreenchange', () => ev++);</script></body>"#,
        );
        eval(&mut s, "v.requestFullscreen()");
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < 300 && eval(&mut s, "ev") != "1" {
            s.tick();
        }
        assert_eq!(s.take_fullscreen(), Some(true));
        assert_eq!(eval(&mut s, "document.fullscreenElement === v && v.matches(':fullscreen')"), "true");
        let _ = s.render();
        assert_eq!(eval(&mut s, "const r = v.getBoundingClientRect(); r.width + 'x' + r.height + '@' + r.top"), "800x600@0");
        // Esc na interface
        s.exit_fullscreen();
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < 300 && eval(&mut s, "ev") != "2" {
            s.tick();
        }
        let _ = s.render();
        assert_eq!(eval(&mut s, "String(document.fullscreenElement) + v.getBoundingClientRect().width"), "null100");
    });
}

#[test]
fn posicao_fixa_nao_rola() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0"><div id=topo style="position:fixed;top:0;left:0;width:100%;height:40px">topo</div>
               <div id=longo style="height:3000px;padding-top:40px">conteúdo</div>
               <script>window.alvo = ''; document.addEventListener('click', e => alvo = e.target.id);</script></body>"#,
        );
        let _ = s.render();
        s.set_scroll(500.0);
        assert_eq!(eval(&mut s, "topo.getBoundingClientRect().top + ',' + longo.getBoundingClientRect().top"), "0,-500");
        // clique na faixa do topo, com a página rolada: acerta o cabeçalho
        s.click(10.0, 510.0);
        assert_eq!(eval(&mut s, "alvo"), "topo");
        s.click(10.0, 600.0);
        assert_eq!(eval(&mut s, "alvo"), "longo");
    });
}

#[test]
fn iframe_mensagens_e_clique() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0"><iframe id=f style="width:200px;height:100px;border:0" srcdoc="
                 <body style='margin:0'><button id=b style='width:200px;height:100px'>x</button><script>
                   window.cliques = 0; b.onclick = () => { cliques++; parent.postMessage({ cliques, topo: top === parent }, '*'); };
                   addEventListener('message', (e) => parent.postMessage('eco:' + e.data, '*'));
                 </script></body>"></iframe>
               <script>window.log = []; addEventListener('message', (e) => log.push(JSON.stringify(e.data)));
                 f.onload = () => log.push('load');</script></body>"#,
        );
        let _ = s.render();
        let wait = |s: &mut Session, n: &str| {
            let t = std::time::Instant::now();
            while t.elapsed().as_millis() < 1500 && eval(s, "log.length") != n {
                s.tick();
                let _ = s.render();
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        wait(&mut s, "1");
        assert_eq!(eval(&mut s, "log.join()"), "load");
        // clique na área do iframe vai para o botão de dentro
        s.click(50.0, 50.0);
        wait(&mut s, "2");
        assert_eq!(eval(&mut s, "log[1]"), r#"{"cliques":1,"topo":true}"#);
        // mensagem da página para o quadro e a resposta
        eval(&mut s, "f.contentWindow.postMessage('oi', '*')");
        wait(&mut s, "3");
        assert_eq!(eval(&mut s, "log[2]"), r#""eco:oi""#);
        // o quadro foi desenhado e aparece na lista de imagens
        assert!(s.video_frames().len() >= 1);
    });
}

#[test]
fn select_abre_lista_e_escolhe() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0"><select id=s style="width:120px;height:24px"><option>Um</option><option disabled>Dois</option><option value=3>Três</option></select>
               <script>window.mudou = ''; s.addEventListener('change', () => mudou = s.value);</script></body>"#,
        );
        let _ = s.render();
        s.click(10.0, 10.0);
        let p = s.take_select_popup().expect("lista");
        assert_eq!(p.labels, vec!["Um", "Dois", "Três"]);
        assert_eq!(p.disabled, vec![false, true, false]);
        assert_eq!(p.selected, 0);
        s.choose_select(p.node, 2);
        assert_eq!(eval(&mut s, "mudou + ',' + s.selectedIndex"), "3,2");
    });
}

#[test]
fn web_crypto_subtle() {
    with_stack(|| {
        let mut s = session("<script>window.r = [];</script>");
        // tudo resolve em microtarefas: o eval já volta com a lista pronta
        eval(
            &mut s,
            r#"(async () => {
                const S = crypto.subtle, txt = (t) => new TextEncoder().encode(t);
                const hex = (b) => [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, '0')).join('');
                r.push(hex(await S.digest('SHA-1', txt('abc'))));
                const h = await S.importKey('raw', txt('Jefe'), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify']);
                const mac = await S.sign('HMAC', h, txt('what do ya want for nothing?'));
                r.push(hex(mac), await S.verify('HMAC', h, mac, new Uint8Array(1)));
                const k = await S.generateKey({ name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']);
                const iv = crypto.getRandomValues(new Uint8Array(12));
                const c = await S.encrypt({ name: 'AES-GCM', iv }, k, new Uint8Array([1, 2, 3]));
                r.push(c.byteLength, hex(await S.decrypt({ name: 'AES-GCM', iv }, k, c)));
                const p = await S.generateKey({ name: 'ECDSA', namedCurve: 'P-384' }, false, ['sign', 'verify']);
                const sig = await S.sign({ name: 'ECDSA', hash: 'SHA-384' }, p.privateKey, c);
                r.push(sig.byteLength, await S.verify({ name: 'ECDSA', hash: 'SHA-384' }, p.publicKey, sig, c));
                const kw = await S.importKey('raw', new Uint8Array(16), 'AES-KW', true, ['wrapKey']);
                r.push(JSON.stringify(await S.exportKey('jwk', kw)), kw instanceof CryptoKey, String(kw));
                await S.digest('MD5', c).catch((e) => r.push(e.name + ': ' + e.message));
                await S.exportKey('raw', p.privateKey).catch((e) => r.push(e.name));
            })()"#,
        );
        assert_eq!(
            eval(&mut s, "r.join(' | ')"),
            "a9993e364706816aba3e25717850c26c9cd0d89d | \
             5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843 | false | 19 | 010203 | 96 | true | \
             {\"alg\":\"A128KW\",\"ext\":true,\"k\":\"AAAAAAAAAAAAAAAAAAAAAA\",\"key_ops\":[\"wrapKey\"],\"kty\":\"oct\"} | \
             true | [object CryptoKey] | \
             NotSupportedError: Failed to execute 'digest' on 'SubtleCrypto': Algorithm: Unrecognized name | \
             InvalidAccessError"
        );
        // getRandomValues vem do sistema e recusa arrays de ponto flutuante
        assert_eq!(eval(&mut s, "crypto.getRandomValues(new Uint8Array(64)).some((x) => x)"), "true");
        assert_eq!(
            eval(&mut s, "try { crypto.getRandomValues(new Float64Array(1)) } catch (e) { e.name }"),
            "TypeMismatchError"
        );
    });
}

/// Mede a criação de sessões (prelúdio incluído): `cargo test --release
/// medir_criacao_de_sessao -- --ignored --nocapture`.
#[test]
#[ignore]
fn medir_criacao_de_sessao() {
    with_stack(|| {
        let fonts = Rc::new(Fonts::system().expect("fontes do sistema"));
        let url = Url::parse("https://teste.ferro/pagina").unwrap();
        let nova = || {
            let loaded = Loaded {
                url: url.clone(),
                base: url.clone(),
                status: 200,
                doc: crate::html::parse("<p>oi</p>"),
                css: Vec::new(),
                title: String::new(),
                sheet_texts: Default::default(),
            };
            let t = std::time::Instant::now();
            let s = Session::new(loaded, Net::new(), fonts.clone(), 800.0, 600.0);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            drop(s);
            ms
        };
        let primeira = nova();
        let resto: Vec<f64> = (0..20).map(|_| nova()).collect();
        let mut ord = resto.clone();
        ord.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!(
            "[medida] sessão: primeira {primeira:.1} ms; seguintes: mediana {:.1} ms, mín {:.1} ms, média {:.1} ms",
            ord[ord.len() / 2],
            ord[0],
            resto.iter().sum::<f64>() / resto.len() as f64
        );
    });
}

#[test]
fn handler_onx_na_ordem_dos_listeners() {
    with_stack(|| {
        // como no Chrome: o handler entra na lista no primeiro `onX = f`;
        // trocar a função mantém o lugar; tirar e pôr de novo vai para o fim
        let mut s = session(
            r#"<div id=d></div><script>
              window.log = [];
              d.addEventListener('click', () => log.push('antes'));
              d.onclick = () => log.push('onclick');
              d.addEventListener('click', () => log.push('depois'));
              d.onclick = () => log.push('onclick2');
              d.click();
              d.onclick = null; d.onclick = () => log.push('fim');
              d.click();
              window.onmessage = () => log.push('w-onmessage');
              addEventListener('message', () => log.push('w-listener'));
              dispatchEvent(new MessageEvent('message'));
            </script>"#,
        );
        assert_eq!(eval(&mut s, "log.join()"), "antes,onclick2,depois,antes,depois,fim,w-onmessage,w-listener");
    });
}

/// Roda o laço da sessão até `cond` (JS) dar true ou o tempo acabar.
fn esperar(s: &mut Session, cond: &str) {
    let t = std::time::Instant::now();
    while t.elapsed().as_millis() < 4000 && eval(s, cond) != "true" {
        s.tick();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn worker_mensagens_e_transferencia() {
    with_stack(|| {
        let mut s = session(
            r#"<script>
              window.log = [];
              const src = `onmessage = (e) => {
                const d = e.data;
                if (d.buf) { const n = new Uint8Array(d.buf); postMessage({ soma: n[0] + n[1] }); postMessage(d.buf, [d.buf]); return; }
                postMessage({ eco: d.a, tipo: typeof window, nome: self.name, data: d.d instanceof Date, mapa: d.m.get('k') });
              };`;
              const w = new Worker(URL.createObjectURL(new Blob([src])), { name: 'eco' });
              w.onmessage = (e) => log.push(e.data instanceof ArrayBuffer ? 'buf:' + e.data.byteLength : JSON.stringify(e.data));
              w.postMessage({ a: [1, 2], d: new Date(0), m: new Map([['k', 'v']]) });
              const b = new Uint8Array([3, 4]).buffer;
              w.postMessage({ buf: b }, [b]);
              log.push('destacado:' + b.byteLength);
            </script>"#,
        );
        esperar(&mut s, "log.length >= 4");
        assert_eq!(
            eval(&mut s, "log.join(' | ')"),
            r#"destacado:0 | {"eco":[1,2],"tipo":"undefined","nome":"eco","data":true,"mapa":"v"} | {"soma":7} | buf:2"#
        );
        // clone que não dá: DataCloneError, como no Chrome
        assert_eq!(
            eval(&mut s, "try { new Worker('data:,').postMessage(() => 1) } catch (e) { e.name + ': ' + e.message }"),
            "DataCloneError: Failed to execute 'postMessage' on 'Worker': () => 1 could not be cloned."
        );
    });
}

#[test]
fn worker_erros_importscripts_e_modulo() {
    with_stack(|| {
        let mut s = session(
            r#"<script>
              window.log = [];
              const url = (t) => URL.createObjectURL(new Blob([t], { type: 'text/javascript' }));
              const lib = url('self.LIB = 40;');
              // importScripts com blob; erro de sintaxe dentro dele vira exceção
              const w = new Worker(url(`importScripts(${JSON.stringify(lib)});
                try { importScripts(${JSON.stringify(url('var = ;'))}); } catch (e) { postMessage('pegou ' + e.name); }
                postMessage('lib ' + (LIB + 2));
                const x = 1;
                throw new TypeError('ruim');`));
              w.onmessage = (e) => log.push(e.data);
              w.onerror = (e) => { log.push(['erro', e.constructor.name, e.message, e.lineno].join(',')); e.preventDefault(); };
              // módulo: import estático (data:) e dinâmico
              const m = new Worker(url(`import { v } from 'data:text/javascript,export const v = 5;';
                const d = await import('data:text/javascript,export default 6;');
                postMessage('modulo ' + (v + d.default));`), { type: 'module' });
              m.onmessage = (e) => log.push(e.data);
              // blob revogado: o script não carrega (Event simples)
              const r = url('1'); URL.revokeObjectURL(r);
              new Worker(r).onerror = (e) => log.push('nao carregou ' + e.constructor.name);
            </script>"#,
        );
        esperar(&mut s, "log.length >= 5");
        assert_eq!(
            eval(&mut s, "log.slice().sort().join(' | ')"),
            "erro,ErrorEvent,Uncaught TypeError: ruim,5 | lib 42 | modulo 11 | nao carregou Event | pegou SyntaxError"
        );
    });
}

#[test]
fn worker_terminate_close_e_fim_das_threads() {
    with_stack(|| {
        let mut s = session(
            r#"<script>
              window.log = [];
              const url = (t) => URL.createObjectURL(new Blob([t]));
              // laço sem fim (com try/finally): terminate() para mesmo assim
              window.ocupado = new Worker(url("postMessage('rodando'); for (;;) { try { } finally { } }"));
              ocupado.onmessage = (e) => { log.push(e.data); ocupado.terminate(); };
              // close(): o que veio antes chega, timer depois não
              const f = new Worker(url("postMessage('a'); setTimeout(() => postMessage('timer'), 0); close(); postMessage('b');"));
              f.onmessage = (e) => log.push(e.data);
              // um que fica vivo para sempre (some quando a sessão acaba)
              window.eterno = new Worker(url('setInterval(() => {}, 5)'));
            </script>"#,
        );
        esperar(&mut s, "log.length >= 3");
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < 300 {
            s.tick();
        }
        assert_eq!(eval(&mut s, "log.slice().sort().join()"), "a,b,rodando");
        // as threads do ocupado e do que fechou acabaram (o laço já juntou)
        let pronto = |s: &Session| {
            let hb = s.host.borrow();
            hb.mailbox.workers.len() == 1 && hb.mailbox.dying.is_empty()
        };
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < 3000 && !pronto(&s) {
            s.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(pronto(&s), "workers parados ainda vivos");
        // a página vai embora: o que sobrou para e a thread acaba
        let threads: Vec<_> = s.host.borrow_mut().mailbox.workers.values_mut().filter_map(|w| w.thread.take()).collect();
        assert_eq!(threads.len(), 1);
        drop(s);
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < 3000 && !threads.iter().all(|h| h.is_finished()) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(threads.iter().all(|h| h.is_finished()), "thread do worker não acabou");
    });
}

#[test]
fn portas_e_broadcast_entre_threads() {
    with_stack(|| {
        let mut s = session(
            r#"<script>
              window.log = [];
              const w = new Worker(URL.createObjectURL(new Blob([`
                onmessage = (e) => {
                  const p = e.ports[0];
                  p.onmessage = (ev) => p.postMessage('worker: ' + ev.data);
                  const bc = new BroadcastChannel('c');
                  bc.onmessage = (ev) => bc.postMessage('bc: ' + ev.data);
                  postMessage('pronto');
                };`])));
              const ch = new MessageChannel();
              ch.port1.onmessage = (e) => log.push(e.data);
              const bc = new BroadcastChannel('c');
              bc.onmessage = (e) => log.push(e.data);
              w.onmessage = () => { ch.port1.postMessage('oi'); bc.postMessage('ola'); };
              w.postMessage(null, [ch.port2]);
              // a porta transferida não vale mais aqui (postMessage não faz nada)
              ch.port2.postMessage('morta');
              // na mesma realm: só entrega depois de start()
              const local = new MessageChannel();
              local.port2.addEventListener('message', (e) => log.push('local: ' + e.data));
              local.port1.postMessage('x');
              setTimeout(() => local.port2.start(), 50);
            </script>"#,
        );
        esperar(&mut s, "log.length >= 3");
        assert_eq!(eval(&mut s, "log.slice().sort().join(' | ')"), "bc: ola | local: x | worker: oi");
    });
}
