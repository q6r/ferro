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

