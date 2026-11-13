//! Geometria das caixas (getBoundingClientRect) em casos conferidos com o
//! Edge: elementos substituídos, grid, tabelas e elementos em linha.

use super::tests::{eval, session, with_stack};

/// "x,y,largura x altura" de cada id, arredondados.
fn caixas(s: &mut super::Session, ids: &[&str]) -> String {
    let lista = ids.iter().map(|i| format!("'{i}'")).collect::<Vec<_>>().join(",");
    eval(
        s,
        &format!(
            "[{lista}].map(i => {{ const r = document.getElementById(i).getBoundingClientRect(); \
             return i + ':' + [r.x, r.y].map(Math.round) + ',' + Math.round(r.width) + 'x' + Math.round(r.height); }}).join(' ')"
        ),
    )
}

#[test]
fn svg_com_altura_em_porcentagem() {
    with_stack(|| {
        let mut s = session(
            r#"<style>
                 body { margin: 0 }
                 .nav { display: flex; justify-content: center; align-items: center; min-height: 40px; width: 800px }
                 .h1 { display: grid; place-items: center; height: 26px; margin: 0 }
                 .h1 svg { width: auto; height: 100% }
               </style>
               <div class="nav"><h1 class="h1" id="h1"><svg id="s1" style="display:block" viewBox="0 0 104 27"></svg></h1></div>
               <div style="height:50px"><svg id="s2" viewBox="0 0 2 1" style="display:block;height:100%"></svg></div>
               <div style="height:40px"><svg id="s3" viewBox="0 0 2 1" style="height:100%"></svg>x</div>
               <div style="height:30px;display:flex"><svg id="s4" viewBox="0 0 3 1" style="height:100%"></svg></div>
               <div style="width:200px"><svg id="s6" viewBox="0 0 4 1" style="height:100%"></svg></div>
               <div style="height:60px;display:grid;place-items:center;width:300px"><svg id="s7" viewBox="0 0 2 1" style="height:50%;width:auto"></svg></div>"#,
        );
        let r = caixas(&mut s, &["h1", "s1", "s2", "s3", "s4", "s6", "s7"]);
        // como no Edge: a largura vem da altura pela proporção do viewBox
        assert_eq!(
            r,
            "h1:350,7,100x26 s1:350,7,100x26 s2:0,40,100x50 s3:0,90,80x40 s4:0,130,90x30 s6:0,160,200x50 s7:120,229,60x30"
        );
    });
}

#[test]
fn substituido_de_bloco_sem_linha_de_texto() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0;width:600px;font:16px/24px sans-serif">
               <div id="d1"><svg id="a1" width="100" height="40" style="display:block;margin:0 auto"></svg></div>
               <div id="d2" style="text-align:center"><svg id="a2" width="80" height="30" style="display:block"></svg></div>
               <div id="d3"><svg id="a3" width="50" height="20" style="display:block;margin-left:auto;margin-right:10px"></svg></div>
               <div id="d4" style="display:flex"><svg id="a4" width="30" height="30"></svg></div>
               <a id="d5" href="/x"><svg id="a5" width="40" height="40" style="display:block;margin:6px 0"></svg></a>
               </body>"#,
        );
        // sem o espaço do texto embaixo; margem auto centraliza e text-align não conta
        assert_eq!(
            caixas(&mut s, &["d1", "a1", "d2", "a2", "d3", "a3", "d4", "a4", "a5"]),
            "d1:0,0,600x40 a1:250,0,100x40 d2:0,40,600x30 a2:0,40,80x30 d3:0,70,600x20 a3:540,70,50x20 \
             d4:0,90,600x30 a4:0,90,30x30 a5:0,126,40x40"
        );
        // o <a> em volta de um elemento só de caixas também tem retângulo
        assert_eq!(eval(&mut s, "d5.getBoundingClientRect().height > 0"), "true");
    });
}

#[test]
fn grid_encolhido_soma_as_colunas() {
    with_stack(|| {
        let mut s = session(
            r#"<style>
                 body { margin: 0 } i { display: block; height: 10px }
                 .f { float: left; clear: both; display: grid }
               </style>
               <div class="f" id="a" style="grid-template-columns:auto auto;gap:10px"><i style="width:30px"></i><i style="width:50px"></i></div>
               <div class="f" id="b" style="grid-template-columns:1fr 1fr"><i style="width:20px"></i><i style="width:60px"></i></div>
               <div class="f" id="c" style="grid-template-columns:100px auto"><i></i><i style="width:40px"></i></div>
               <div class="f" id="d" style="grid-template-columns:repeat(auto-fill,50px)"><i></i><i></i><i></i></div>
               <div class="f" id="e" style="grid-auto-flow:column;gap:8px"><i style="width:10px"></i><i style="width:20px"></i><i style="width:30px"></i></div>"#,
        );
        // fr no max-content: 1fr cabe o maior (60) nas duas colunas
        assert_eq!(
            caixas(&mut s, &["a", "b", "c", "d", "e"]),
            "a:0,0,90x10 b:0,10,120x10 c:0,20,140x10 d:0,30,50x30 e:0,60,76x10"
        );
    });
}

#[test]
fn grid_auto_flow_column_e_dense() {
    with_stack(|| {
        let mut s = session(
            r#"<style>body { margin: 0; width: 800px } i { display: block; height: 18px } .g { display: grid }</style>
               <div class="g" style="grid-auto-flow:column;grid-auto-columns:120px;gap:8px"><i id="a1"></i><i id="a2"></i><i id="a3"></i></div>
               <div class="g" style="grid-auto-flow:column;grid-template-rows:20px 20px"><i id="b1"></i><i id="b2"></i><i id="b3"></i></div>
               <div class="g" style="grid-template-columns:repeat(3,100px);grid-auto-flow:row dense"><i id="c1" style="grid-column:span 2"></i><i id="c2" style="grid-column:span 2"></i><i id="c3"></i></div>
               <div class="g" style="grid-template-columns:100px 1fr;grid-template-rows:20px 20px 20px"><i id="f1" style="grid-row:1/-1;height:auto"></i><i id="f2"></i></div>"#,
        );
        assert_eq!(
            caixas(&mut s, &["a1", "a2", "a3", "b1", "b2", "b3", "c1", "c2", "c3", "f1", "f2"]),
            "a1:0,0,120x18 a2:128,0,120x18 a3:256,0,120x18 b1:0,18,400x18 b2:0,38,400x18 b3:400,18,400x18 \
             c1:0,58,200x18 c2:0,76,200x18 c3:200,58,100x18 f1:0,94,100x60 f2:100,94,700x18"
        );
    });
}

#[test]
fn tabela_cellpadding_e_linha_vazia() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0">
               <table id="t" border="0" cellpadding="0" cellspacing="0">
                 <tr id="r1"><td id="v1" valign="top"><div style="width:10px;height:10px;margin:3px 2px 6px"></div></td><td id="c1"><i style="display:block;width:50px;height:12px"></i></td></tr>
                 <tr id="r2" style="height:5px"></tr>
                 <tr id="r3"><td id="v3"><i style="display:block;width:20px;height:10px"></i></td></tr>
               </table></body>"#,
        );
        // cellpadding=0 tira o padding de 1px; a linha vazia ocupa os 5px; a
        // célula tem a altura da linha (valores do Edge)
        assert_eq!(
            caixas(&mut s, &["t", "r1", "v1", "c1", "r2", "r3", "v3"]),
            "t:0,0,70x34 r1:0,0,70x19 v1:0,0,20x19 c1:20,0,50x19 r2:0,19,70x5 r3:0,24,70x10 v3:0,24,20x10"
        );
    });
}

#[test]
fn svg_com_padding_e_borda() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0">
               <div><svg id="a" width="30" height="30" style="border:2px solid #000;padding:3px"></svg></div>
               <div><svg id="b" width="40" height="20" style="box-sizing:border-box;border:4px solid green;display:block"></svg></div>
               </body>"#,
        );
        assert_eq!(caixas(&mut s, &["a", "b"]).split(' ').map(|p| p.rsplit(',').next().unwrap()).collect::<Vec<_>>(), ["40x40", "40x20"]);
    });
}

#[test]
fn grid_alinhamento_e_linhas_que_preenchem() {
    with_stack(|| {
        let mut s = session(
            r#"<style>body { margin: 0; width: 800px } i { display: block; height: 18px }
                 .g { display: grid; grid-template-columns: 200px 200px; height: 50px }</style>
               <div class="g" style="place-items:center"><i id="a1" style="width:22px"></i><i id="a2" style="width:50px"></i></div>
               <div class="g" style="justify-items:end;align-items:start"><i id="b1" style="width:22px"></i><i id="b2" style="justify-self:start;width:29px"></i></div>
               <div style="display:grid;grid-template-rows:auto 1fr auto;height:100px"><i id="c1"></i><i id="c2" style="height:auto"></i><i id="c3"></i></div>
               <div style="display:grid;min-height:60px"><i id="d1" style="height:auto"></i><i id="d2" style="height:auto"></i></div>"#,
        );
        // a sobra da altura vai para a linha 1fr; sem fr, as auto esticam (até com min-height)
        assert_eq!(
            caixas(&mut s, &["a1", "a2", "b1", "b2", "c1", "c2", "c3", "d1", "d2"]),
            "a1:89,16,22x18 a2:275,16,50x18 b1:178,50,22x18 b2:200,50,29x18 c1:0,100,800x18 c2:0,118,800x64 \
             c3:0,182,800x18 d1:0,200,800x30 d2:0,230,800x30"
        );
    });
}

#[test]
fn grid_posiciona_definidos_primeiro() {
    with_stack(|| {
        // a estrutura do feed do g1: a imagem e a lista têm posição definida e
        // vão primeiro; o cabeçalho e o título caem na coluna 2, e os
        // metadados, presos à coluna 2, descem para depois da lista
        let mut s = session(
            r#"<style>body { margin: 0; width: 800px } i { display: block; height: 20px }
                 .fb { align-items: start; column-gap: 16px; display: grid;
                       grid-template-columns: repeat(auto-fit, minmax(45%, max-content)); grid-template-rows: repeat(5, auto) }</style>
               <div class="fb"><i id="hd" style="height:8px"></i><i id="ti" style="height:60px"></i>
                 <i id="mw" style="grid-column:1;grid-row:1/span 5;height:100px"></i>
                 <i id="me" style="grid-column:2;margin:6px 0 0"></i><i id="re" style="grid-column:2;grid-row:3;height:40px"></i></div>
               <div style="display:grid;grid-template-columns:repeat(3,50px)"><i id="a"></i><i id="b" style="grid-row:2"></i><i id="c"></i><i id="d" style="grid-row:2"></i></div>"#,
        );
        assert_eq!(
            caixas(&mut s, &["hd", "ti", "mw", "me", "re", "a", "b", "c", "d"]),
            "hd:376,0,360x8 ti:376,8,360x60 mw:0,0,360x100 me:376,114,360x20 re:376,68,360x40 \
             a:0,134,50x20 b:0,154,50x20 c:50,134,50x20 d:50,154,50x20"
        );
    });
}

#[test]
fn letter_spacing_e_word_spacing() {
    with_stack(|| {
        let mut s = session(
            r#"<body style="margin:0;font-size:20px">
               <span id="a" style="display:inline-block">Microsoft says</span><br>
               <span id="b" style="display:inline-block;letter-spacing:2px">Microsoft says</span><br>
               <span id="c" style="display:inline-block;word-spacing:10px">Microsoft says</span><br>
               <div style="letter-spacing:0.1em;font-size:10px"><span id="d" style="display:inline-block;font-size:20px">Microsoft says</span></div>
               </body>"#,
        );
        // depois de cada um dos 14 caracteres; word-spacing só no espaço; em
        // herdado já em px (0,1em de 10px)
        let w = |s: &mut super::Session, id: &str| eval(s, &format!("{id}.getBoundingClientRect().width"));
        let base: f64 = w(&mut s, "a").parse().unwrap();
        let diff = |s: &mut super::Session, id: &str| (w(s, id).parse::<f64>().unwrap() - base).round();
        assert_eq!(diff(&mut s, "b"), 28.0);
        assert_eq!(diff(&mut s, "c"), 10.0);
        assert_eq!(diff(&mut s, "d"), 14.0);
    });
}
