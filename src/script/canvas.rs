//! Funções nativas do Canvas 2D para o prelúdio (o estado do contexto —
//! estilos, transformação, caminho — fica no JS; o desenho é aqui, em
//! `crate::canvas`).

use std::sync::Arc;

use ferro_js::value::{CallArgs, Kind, ObjRef, PropKey, TaKind};
use ferro_js::{Value, Vm};

use super::{Host, host, node_of};
use crate::canvas::{Bitmap, Cap, Join, Mat, Op, Paint, Rgba, StrokeStyle, Style, decode_path};
use crate::dom::NodeId;
use crate::fonts::{FontId, Fonts};
use crate::paint::ImageData;
use crate::style::{Color, FontFamily};

type R = Result<Value, Value>;

fn read<T>(vm: &Vm, f: impl FnOnce(&Host) -> T) -> T {
    let h = host(vm);
    let b = h.borrow();
    f(&b)
}

fn write<T>(vm: &Vm, f: impl FnOnce(&mut Host) -> T) -> T {
    let h = host(vm);
    let mut b = h.borrow_mut();
    f(&mut b)
}

fn num(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<f32, Value> {
    Ok(vm.to_number(&a.arg(i))? as f32)
}

fn text(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<String, Value> {
    vm.to_rust_string(&a.arg(i))
}

fn el(vm: &mut Vm, a: &CallArgs) -> Result<NodeId, Value> {
    node_of(&a.arg(0)).ok_or_else(|| vm.type_error("Illegal invocation"))
}

fn get(vm: &mut Vm, v: &Value, k: &str) -> Result<Value, Value> {
    vm.get(v, &PropKey::str(k))
}

fn numbers(v: &Value) -> Vec<f64> {
    let bytes = super::web::bytes_of(v).unwrap_or_default();
    bytes.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect()
}

/// Lê um array JS de números.
fn array_nums(vm: &mut Vm, v: &Value) -> Result<Vec<f32>, Value> {
    let lv = get(vm, v, "length")?;
    let len = vm.to_number(&lv)? as usize;
    let mut out = Vec::with_capacity(len.min(4096));
    for i in 0..len.min(4096) {
        let x = vm.get_index(v, &Value::Number(i as f64))?;
        out.push(vm.to_number(&x)? as f32);
    }
    Ok(out)
}

fn matrix(vm: &mut Vm, v: &Value) -> Result<Mat, Value> {
    let n = array_nums(vm, v)?;
    if n.len() < 6 {
        return Ok(Mat::IDENTITY);
    }
    Ok(Mat([n[0], n[1], n[2], n[3], n[4], n[5]]))
}

fn rgba(c: Color) -> Rgba {
    [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, c.a as f32 / 255.0]
}

fn parse_color(s: &str) -> Option<Rgba> {
    crate::style::parse_color(s.trim(), Color::BLACK).map(rgba)
}

/// A imagem atual de uma fonte de `drawImage`/`createPattern`: `<img>`,
/// `<canvas>`, `<video>` ou um ImageBitmap (objeto com `_source`).
fn source_image(vm: &mut Vm, v: &Value) -> Result<Option<Arc<ImageData>>, Value> {
    let v = match node_of(v) {
        Some(_) => v.clone(),
        None => {
            let s = get(vm, v, "_source")?;
            if let Value::Number(id) = s {
                return Ok(read(vm, |h| h.bitmaps.get(&(id as u32)).cloned()));
            }
            s
        }
    };
    let Some(n) = node_of(&v) else { return Ok(None) };
    Ok(read(vm, |h| image_of_node(h, n)))
}

pub(crate) fn image_of_node(h: &Host, n: NodeId) -> Option<Arc<ImageData>> {
    let el = h.page.doc.element(n)?;
    match el.tag.as_str() {
        "canvas" => h.canvases.get(&n).map(|b| Arc::new(b.snapshot())),
        "video" => h.media.players.get(&n).and_then(|p| p.pipeline.shown.as_ref().map(|(_, i)| i.clone())),
        _ => {
            if let Some(i) = h.images.nodes.get(&n) {
                return Some(i.clone());
            }
            let src = el.attr("currentSrc").or(el.attr("src"))?;
            let url = h.page.base.join(src.trim()).ok()?;
            h.image_cache.get(&url).cloned().flatten()
        }
    }
}

fn stops(vm: &mut Vm, g: &Value) -> Result<Vec<(f32, Rgba)>, Value> {
    let list = get(vm, g, "_stops")?;
    let lv = get(vm, &list, "length")?;
    let len = vm.to_number(&lv)? as usize;
    let mut out = Vec::new();
    for i in 0..len.min(256) {
        let s = vm.get_index(&list, &Value::Number(i as f64))?;
        let ov = vm.get_index(&s, &Value::Number(0.0))?;
        let off = vm.to_number(&ov)? as f32;
        let c = vm.get_index(&s, &Value::Number(1.0))?;
        let c = vm.to_rust_string(&c)?;
        if let Some(c) = parse_color(&c) {
            out.push((off.clamp(0.0, 1.0), c));
        }
    }
    // estáveis por posição (paradas na mesma posição mantêm a ordem)
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(out)
}

/// O estilo (cor, gradiente ou padrão) de fillStyle/strokeStyle.
fn style(vm: &mut Vm, v: &Value) -> Result<Option<Style>, Value> {
    if let Value::String(s) = v {
        return Ok(parse_color(&s.to_rust()).map(Style::Solid));
    }
    if !matches!(v, Value::Object(_)) {
        return Ok(None);
    }
    let ty = get(vm, v, "_type")?;
    let ty = vm.to_rust_string(&ty)?;
    let args = get(vm, v, "_args")?;
    let n = array_nums(vm, &args)?;
    let n = |i: usize| n.get(i).copied().unwrap_or(0.0);
    Ok(Some(match ty.as_str() {
        "linear" => Style::Linear { p0: (n(0), n(1)), p1: (n(2), n(3)), stops: stops(vm, v)? },
        "radial" => Style::Radial { c0: (n(0), n(1)), r0: n(2), c1: (n(3), n(4)), r1: n(5), stops: stops(vm, v)? },
        "conic" => Style::Conic { center: (n(1), n(2)), angle: n(0), stops: stops(vm, v)? },
        "pattern" => {
            let src = get(vm, v, "_img")?;
            let Some(img) = source_image(vm, &src)? else { return Ok(None) };
            let rep = get(vm, v, "_rep")?;
            let rep = vm.to_rust_string(&rep)?;
            Style::Pattern {
                img,
                repeat_x: matches!(rep.as_str(), "" | "repeat" | "repeat-x"),
                repeat_y: matches!(rep.as_str(), "" | "repeat" | "repeat-y"),
            }
        }
        _ => return Ok(None),
    }))
}

fn paint(vm: &mut Vm, style_v: &Value, m: Mat, alpha: f32, op: &str) -> Result<Option<Paint>, Value> {
    let Some(style) = style(vm, style_v)? else { return Ok(None) };
    Ok(Some(Paint { style, inv: m.invert().unwrap_or(Mat::IDENTITY), alpha: alpha.clamp(0.0, 1.0), op: Op::parse(op) }))
}

fn with_bitmap<T>(vm: &Vm, n: NodeId, f: impl FnOnce(&mut Bitmap) -> T) -> Option<T> {
    write(vm, |h| {
        let r = h.canvases.get_mut(&n).map(f);
        if r.is_some() {
            h.canvas_dirty.insert(n);
        }
        r
    })
}

/// `cvSize(el, w, h)`: cria ou redimensiona (apaga) a imagem.
fn size(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let (w, h) = (num(vm, a, 1)?.max(0.0) as usize, num(vm, a, 2)?.max(0.0) as usize);
    write(vm, |hb| {
        hb.canvases.entry(n).or_insert_with(|| Bitmap::new(0, 0)).resize(w, h);
        hb.canvas_dirty.insert(n);
    });
    Ok(Value::Undefined)
}

/// `cvFill(el, caminho, evenodd, estilo, matriz, alfa, op)`.
fn fill(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let path = decode_path(&numbers(&a.arg(1)));
    let evenodd = ferro_js::vm::truthy(&a.arg(2));
    let m = matrix(vm, &a.arg(4))?;
    let alpha = num(vm, a, 5)?;
    let op = text(vm, a, 6)?;
    if let Some(p) = paint(vm, &a.arg(3), m, alpha, &op)? {
        with_bitmap(vm, n, |b| b.fill_path(&path, evenodd, &p));
    }
    Ok(Value::Undefined)
}

/// `cvStroke(el, caminho, estilo, matriz, alfa, op, largura, ponta, junção, limite, traços, deslocamento)`.
fn stroke(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let path = decode_path(&numbers(&a.arg(1)));
    let m = matrix(vm, &a.arg(3))?;
    let alpha = num(vm, a, 4)?;
    let op = text(vm, a, 5)?;
    let width = num(vm, a, 6)?;
    let cap = match text(vm, a, 7)?.as_str() {
        "round" => Cap::Round,
        "square" => Cap::Square,
        _ => Cap::Butt,
    };
    let join = match text(vm, a, 8)?.as_str() {
        "round" => Join::Round,
        "bevel" => Join::Bevel,
        _ => Join::Miter,
    };
    let miter_limit = num(vm, a, 9)?;
    let dash = array_nums(vm, &a.arg(10))?;
    let dash_offset = num(vm, a, 11)?;
    let st = StrokeStyle { width, cap, join, miter_limit, dash, dash_offset };
    if let Some(p) = paint(vm, &a.arg(2), m, alpha, &op)? {
        with_bitmap(vm, n, |b| b.stroke_path(&path, &st, &p));
    }
    Ok(Value::Undefined)
}

fn clip(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let path = decode_path(&numbers(&a.arg(1)));
    let evenodd = ferro_js::vm::truthy(&a.arg(2));
    write(vm, |h| {
        if let Some(b) = h.canvases.get_mut(&n) {
            b.clip_path(&path, evenodd);
        }
    });
    Ok(Value::Undefined)
}

fn save(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    write(vm, |h| h.canvases.get_mut(&n).map(Bitmap::save));
    Ok(Value::Undefined)
}

fn restore(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    write(vm, |h| h.canvases.get_mut(&n).map(Bitmap::restore));
    Ok(Value::Undefined)
}

fn reset_state(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    write(vm, |h| h.canvases.get_mut(&n).map(Bitmap::reset_state));
    Ok(Value::Undefined)
}

/// `cvClear(el, x, y, w, h)` em coordenadas do dispositivo.
fn clear(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let (x, y, w, h) = (num(vm, a, 1)?, num(vm, a, 2)?, num(vm, a, 3)?, num(vm, a, 4)?);
    with_bitmap(vm, n, |b| b.clear_rect(x, y, w, h));
    Ok(Value::Undefined)
}

/// `cvSourceSize(fonte)`: [largura, altura] (0, 0 se ainda não carregou).
fn source_size(vm: &mut Vm, a: &mut CallArgs) -> R {
    let img = source_image(vm, &a.arg(0))?;
    let (w, h) = img.map_or((0, 0), |i| (i.width, i.height));
    Ok(vm.array_value(vec![Value::Number(w as f64), Value::Number(h as f64)]))
}

/// `cvImage(el, fonte, sx, sy, sw, sh, dx, dy, dw, dh, matriz, alfa, op, suave)`.
fn image(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let Some(img) = source_image(vm, &a.arg(1))? else { return Ok(Value::Bool(false)) };
    let mut v = [0.0f32; 8];
    for (i, x) in v.iter_mut().enumerate() {
        *x = num(vm, a, 2 + i)?;
    }
    let m = matrix(vm, &a.arg(10))?;
    let alpha = num(vm, a, 11)?;
    let op = Op::parse(&text(vm, a, 12)?);
    let smooth = ferro_js::vm::truthy(&a.arg(13));
    with_bitmap(vm, n, |b| {
        b.draw_image(&img, (v[0], v[1], v[2], v[3]), (v[4], v[5], v[6], v[7]), m, alpha.clamp(0.0, 1.0), op, smooth)
    });
    Ok(Value::Bool(true))
}

/// Fonte CSS do canvas ("bold 16px Arial") → (face, tamanho em px).
fn parse_font(fonts: &Fonts, spec: &str) -> (FontId, f32) {
    let mut size = 10.0;
    let mut weight = 400;
    let mut italic = false;
    let mut family_start = None;
    let toks: Vec<&str> = spec.split_whitespace().collect();
    for (i, t) in toks.iter().enumerate() {
        let l = t.to_ascii_lowercase();
        match l.as_str() {
            "italic" | "oblique" => italic = true,
            "bold" | "bolder" => weight = 700,
            "lighter" => weight = 300,
            "normal" | "small-caps" => {}
            _ if l.parse::<u16>().is_ok() && family_start.is_none() && !l.ends_with("px") => {
                weight = l.parse().unwrap_or(400);
            }
            _ => {
                // tamanho (com /altura-da-linha opcional) e, depois, as famílias
                let sz = l.split('/').next().unwrap_or("");
                let parsed = if let Some(v) = sz.strip_suffix("px") {
                    v.parse::<f32>().ok()
                } else if let Some(v) = sz.strip_suffix("pt") {
                    v.parse::<f32>().ok().map(|p| p * 4.0 / 3.0)
                } else if let Some(v) = sz.strip_suffix("rem").or_else(|| sz.strip_suffix("em")) {
                    v.parse::<f32>().ok().map(|e| e * 16.0)
                } else if let Some(v) = sz.strip_suffix('%') {
                    v.parse::<f32>().ok().map(|p| p / 100.0 * 16.0)
                } else {
                    None
                };
                if let Some(s) = parsed {
                    size = s;
                    family_start = Some(i + 1);
                    break;
                }
            }
        }
    }
    let families = family_start.map(|i| toks[i..].join(" ")).unwrap_or_default();
    let names: Vec<String> = families
        .split(',')
        .map(|f| f.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase())
        .filter(|f| !f.is_empty())
        .collect();
    let mut family = FontFamily::Sans;
    for n in &names {
        if let Some(id) = fonts.web_font(n, weight, italic) {
            return (id, size);
        }
        let f = match n.as_str() {
            "serif" | "times" | "times new roman" | "georgia" | "garamond" => Some(FontFamily::Serif),
            "monospace" | "courier" | "courier new" | "consolas" | "menlo" | "monaco" => Some(FontFamily::Mono),
            "system-ui" | "-apple-system" | "segoe ui" | "blinkmacsystemfont" => Some(FontFamily::System),
            "sans-serif" | "arial" | "helvetica" | "roboto" | "verdana" => Some(FontFamily::Sans),
            _ => None,
        };
        if let Some(f) = f {
            family = f;
            break;
        }
    }
    (fonts.pick(family, weight, italic), size.clamp(1.0, 2000.0))
}

/// `cvMeasure(fonte, texto)` → [largura, subida, descida, subida da fonte, descida da fonte].
fn measure(vm: &mut Vm, a: &mut CallArgs) -> R {
    let font = text(vm, a, 0)?;
    let t = text(vm, a, 1)?;
    let fonts = read(vm, |h| h.fonts.clone());
    let (id, size) = parse_font(&fonts, &font);
    let w = fonts.measure(id, size, &t);
    let lm = fonts.line_metrics(id, size);
    let vals = [w, lm.ascent * 0.95, lm.descent * 0.6, lm.ascent, lm.descent];
    Ok(vm.array_value(vals.iter().map(|v| Value::Number(*v as f64)).collect()))
}

/// `cvText(el, texto, x, y, fonte, alinhamento, linha-base, estilo, matriz, alfa, op, largura máxima)`.
fn draw_text(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let t = text(vm, a, 1)?;
    let (x, y) = (num(vm, a, 2)?, num(vm, a, 3)?);
    let font = text(vm, a, 4)?;
    let align = text(vm, a, 5)?;
    let baseline = text(vm, a, 6)?;
    let m = matrix(vm, &a.arg(8))?;
    let alpha = num(vm, a, 9)?;
    let op = text(vm, a, 10)?;
    let max_w = vm.to_number(&a.arg(11))? as f32;
    let Some(p) = paint(vm, &a.arg(7), m, alpha, &op)? else { return Ok(Value::Undefined) };
    let fonts = read(vm, |h| h.fonts.clone());
    let (id, mut size) = parse_font(&fonts, &font);
    let mut w = fonts.measure(id, size, &t);
    if max_w.is_finite() && max_w > 0.0 && w > max_w {
        size *= max_w / w;
        w = max_w;
    }
    let lm = fonts.line_metrics(id, size);
    let dx = match align.as_str() {
        "center" => -w / 2.0,
        "right" | "end" => -w,
        _ => 0.0,
    };
    let dy = match baseline.as_str() {
        "top" => lm.ascent,
        "hanging" => lm.ascent * 0.8,
        "middle" => (lm.ascent - lm.descent) / 2.0,
        "bottom" | "ideographic" => -lm.descent,
        _ => 0.0,
    };
    let (ox, oy) = m.apply(x + dx, y + dy);
    let scale = m.scale().max(0.01);
    let dsize = size * scale;
    // rasteriza os glifos numa máscara (rotação não é suportada: só posição e escala)
    let total = fonts.measure(id, dsize, &t).ceil() as usize + 4;
    let lmd = fonts.line_metrics(id, dsize);
    let (top, bottom) = (lmd.ascent.ceil() as i32 + 2, lmd.descent.ceil() as i32 + 2);
    let mh = (top + bottom).max(1) as usize;
    let mut mask = vec![0u8; total * mh];
    let mut pen = 0.0f32;
    for ch in t.chars() {
        if !ch.is_whitespace()
            && let Some(g) = fonts.glyph(id, ch, dsize)
        {
            let gm = g.metrics;
            let gx = pen.round() as i32 + gm.xmin + 2;
            let gy = top - gm.height as i32 - gm.ymin;
            for row in 0..gm.height {
                let py = gy + row as i32;
                if py < 0 || py >= mh as i32 {
                    continue;
                }
                for col in 0..gm.width {
                    let px = gx + col as i32;
                    if px < 0 || px >= total as i32 {
                        continue;
                    }
                    let c = g.coverage[row * gm.width + col];
                    let i = py as usize * total + px as usize;
                    mask[i] = mask[i].max(c);
                }
            }
        }
        pen += fonts.advance(id, ch, dsize);
    }
    let x0 = ox.round() as i32 - 2;
    let y0 = oy.round() as i32 - top;
    with_bitmap(vm, n, |b| b.draw_mask(x0, y0, total, mh, &mask, &p));
    Ok(Value::Undefined)
}

