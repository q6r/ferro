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

/// `sbAppend(id, bytes, timestampOffset)` → null ou mensagem de erro.
fn sb_append(vm: &mut Vm, a: &mut CallArgs) -> R {
    let sb = id(vm, a, 0)?;
    let bytes = super::web::bytes_of(&a.arg(1)).unwrap_or_default();
    let offset = num(vm, a, 2).unwrap_or(0.0);
    let r = write(vm, |h| h.media.append(sb, &bytes, if offset.is_finite() { offset } else { 0.0 }));
    Ok(match r {
        Ok(()) => Value::Null,
        Err(e) => Value::str(&e),
    })
}

fn sb_remove(vm: &mut Vm, a: &mut CallArgs) -> R {
    let sb = id(vm, a, 0)?;
    let (s, e) = (num(vm, a, 1)?, num(vm, a, 2)?);
    write(vm, |h| h.media.remove(sb, s, e));
    Ok(Value::Undefined)
}

fn sb_buffered(vm: &mut Vm, a: &mut CallArgs) -> R {
    let sb = id(vm, a, 0)?;
    let r = write(vm, |h| h.media.buffers.get(&sb).map(|b| b.buffered()).unwrap_or_default());
    Ok(ranges(vm, r))
}

fn attach(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let ms = id(vm, a, 1)?;
    write(vm, |h| h.media.attach(n, ms));
    Ok(Value::Undefined)
}

fn detach(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    write(vm, |h| h.media.players.remove(&n));
    Ok(Value::Undefined)
}

/// `mediaLoadUrl(el, url)`: baixa um arquivo de mídia inteiro (MP4) para tocar.
fn load_url(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let url = vm.to_rust_string(&a.arg(1))?;
    let Ok(url) = url::Url::parse(&url) else { return Ok(Value::Bool(false)) };
    write(vm, |h| {
        h.media_loading.insert(n, url.clone());
        let net = h.net.clone();
        h.in_flight += 1;
        h.pool.submit(Box::new(move || {
            let bytes = net.fetch(&url).ok().filter(|r| r.status < 400).map(|r| r.body);
            super::Done::MediaFile { node: n, url, bytes }
        }));
    });
    Ok(Value::Bool(true))
}

/// `mediaBuffered(el)`: intervalos com dados de um arquivo comum, em lista plana.
fn buffered(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let ranges = write(vm, |h| {
        let source = h.media.players.get(&n).and_then(|p| p.source);
        source.map(|s| h.media.source_buffered(s)).unwrap_or_default()
    });
    let flat = ranges.iter().flat_map(|&(s, e)| [Value::Number(s), Value::Number(e)]).collect();
    Ok(vm.array_value(flat))
}

fn play(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let ev = write(vm, |h| {
        h.repaint = true;
        h.media.play(n)
    });
    Ok(events(vm, ev))
}

fn pause(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let ev = write(vm, |h| {
        h.repaint = true;
        h.media.pause(n)
    });
    Ok(events(vm, ev))
}

fn seek(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let t = num(vm, a, 1)?;
    let ev = write(vm, |h| h.media.seek(n, if t.is_finite() { t } else { 0.0 }));
    Ok(events(vm, ev))
}

/// `mediaSet(el, rate, volume, muted)`.
fn set(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let (rate, volume) = (num(vm, a, 1)?, num(vm, a, 2)?);
    let muted = ferro_js::vm::truthy(&a.arg(3));
    write(vm, |h| {
        let p = h.media.player(n);
        if rate.is_finite() && rate > 0.0 && (rate - p.rate).abs() > 1e-9 {
            let t = p.current_time();
            p.rate = rate;
            p.seek_base(t);
        }
        p.volume = volume.clamp(0.0, 1.0);
        p.muted = muted;
    });
    Ok(Value::Undefined)
}

/// `[currentTime, readyState, paused, seeking, ended, largura, altura, duração]`.
fn state(vm: &mut Vm, a: &mut CallArgs) -> R {
    let n = el(vm, a)?;
    let v = write(vm, |h| {
        let dur = h
            .media
            .players
            .get(&n)
            .and_then(|p| p.source)
            .and_then(|s| h.media.sources.get(&s))
            .map_or(f64::NAN, |s| s.duration);
        h.media.players.get(&n).map(|p| {
            vec![
                Value::Number(p.current_time()),
                Value::Number(p.ready_state as f64),
                Value::Bool(p.paused),
                Value::Bool(p.seeking),
                Value::Bool(p.ended),
                Value::Number(p.width as f64),
                Value::Number(p.height as f64),
                Value::Number(dur),
                Value::Number(p.pipeline.frames_shown as f64),
            ]
        })
    });
    Ok(match v {
        Some(v) => vm.array_value(v),
        None => Value::Null,
    })
}

pub(crate) fn install(vm: &Vm, n: &ObjRef) {
    let r = &vm.realm;
    for (name, len, f) in [
        ("mediaSupports", 1, supports as ferro_js::value::NativeFn),
        ("msNew", 0, ms_new),
        ("msDuration", 2, ms_duration),
        ("msEnd", 1, ms_end),
        ("msBuffered", 1, ms_buffered),
        ("msDescribe", 1, ms_describe),
        ("sbNew", 2, sb_new),
        ("sbAppend", 3, sb_append),
        ("sbRemove", 3, sb_remove),
        ("sbBuffered", 1, sb_buffered),
        ("mediaAttach", 2, attach),
        ("mediaLoadUrl", 2, load_url),
        ("mediaBuffered", 1, buffered),
        ("mediaDetach", 1, detach),
        ("mediaPlay", 1, play),
        ("mediaPause", 1, pause),
        ("mediaSeek", 2, seek),
        ("mediaSet", 4, set),
        ("mediaState", 1, state),
    ] {
        r.method(n, name, len, f);
    }
}
