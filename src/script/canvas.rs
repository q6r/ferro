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

