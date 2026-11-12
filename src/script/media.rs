//! Funções nativas de mídia para o prelúdio: MediaSource, SourceBuffer e o
//! estado de reprodução dos `<video>`/`<audio>` (ver `crate::media`).

use ferro_js::value::{CallArgs, ObjRef};
use ferro_js::{Value, Vm};

use super::{Host, host, node_of};

type R = Result<Value, Value>;

fn write<T>(vm: &Vm, f: impl FnOnce(&mut Host) -> T) -> T {
    let h = host(vm);
    let mut b = h.borrow_mut();
    f(&mut b)
}

fn num(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<f64, Value> {
    vm.to_number(&a.arg(i))
}

fn id(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<u32, Value> {
    Ok(num(vm, a, i)? as u32)
}

fn el(vm: &mut Vm, a: &CallArgs) -> Result<usize, Value> {
    node_of(&a.arg(0)).ok_or_else(|| vm.type_error("Illegal invocation"))
}

fn events(vm: &mut Vm, ev: Vec<&'static str>) -> Value {
    let items = ev.into_iter().map(Value::str).collect();
    vm.array_value(items)
}

fn ranges(vm: &mut Vm, r: Vec<(f64, f64)>) -> Value {
    let flat: Vec<Value> = r.into_iter().flat_map(|(s, e)| [Value::Number(s), Value::Number(e)]).collect();
    vm.array_value(flat)
}

fn supports(vm: &mut Vm, a: &mut CallArgs) -> R {
    let t = vm.to_rust_string(&a.arg(0))?;
    let ok = crate::media::is_type_supported(&t);
    if std::env::var_os("FERRO_MIDIA_LOG").is_some() {
        eprintln!("[mídia] suporta {t:?}? {ok}");
    }
    Ok(Value::Bool(ok))
}

fn ms_new(vm: &mut Vm, _a: &mut CallArgs) -> R {
    Ok(Value::Number(write(vm, |h| h.media.new_source()) as f64))
}

/// `msDuration(id)` lê; `msDuration(id, d)` define.
fn ms_duration(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ms = id(vm, a, 0)?;
    let set = if a.args.len() > 1 { Some(num(vm, a, 1)?) } else { None };
    let d = write(vm, |h| {
        let s = h.media.sources.get_mut(&ms)?;
        if let Some(d) = set {
            s.duration = d;
        }
        Some(s.duration)
    });
    Ok(Value::Number(d.unwrap_or(f64::NAN)))
}

fn ms_end(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ms = id(vm, a, 0)?;
    write(vm, |h| {
        // sem duração definida, vale o fim do que foi carregado
        let end = h.media.source_buffered(ms).last().map(|r| r.1);
        if let Some(s) = h.media.sources.get_mut(&ms) {
            s.ended = true;
            if !s.duration.is_finite()
                && let Some(e) = end
            {
                s.duration = e;
            }
        }
    });
    Ok(Value::Undefined)
}

fn ms_buffered(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ms = id(vm, a, 0)?;
    let r = write(vm, |h| h.media.source_buffered(ms));
    Ok(ranges(vm, r))
}

fn ms_describe(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ms = id(vm, a, 0)?;
    Ok(Value::str(&write(vm, |h| h.media.describe(ms))))
}

/// `sbNew(fonte, tipo)` → id ou string de erro.
fn sb_new(vm: &mut Vm, a: &mut CallArgs) -> R {
    let ms = id(vm, a, 0)?;
    let mime = vm.to_rust_string(&a.arg(1))?;
    Ok(match write(vm, |h| h.media.add_buffer(ms, &mime)) {
        Ok(i) => Value::Number(i as f64),
        Err(e) => Value::str(&e),
    })
}

