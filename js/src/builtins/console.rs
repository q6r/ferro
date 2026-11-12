//! console e a formatação de valores no estilo do Node (`inspect`).

use super::Realm;
use crate::conv::number_to_string;
use crate::ops::data_in_chain;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Out, Vm};

pub fn install(r: &Realm) {
    let c = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    for (name, f) in [
        ("log", log as NativeFn),
        ("info", log),
        ("debug", log),
        ("warn", warn),
        ("error", warn),
        ("trace", trace),
        ("dir", dir),
        ("dirxml", log),
        ("table", log),
        ("assert", assert),
        ("time", time),
        ("timeEnd", time_end),
        ("timeLog", time_log),
        ("count", count),
        ("countReset", count_reset),
        ("group", group),
        ("groupCollapsed", group),
        ("groupEnd", group_end),
        ("clear", clear),
    ] {
        r.method(&c, name, 0, f);
    }
    r.global_value("console", Value::Object(c));
}

fn emit(vm: &mut Vm, kind: Out, text: &str) {
    let pad = " ".repeat(vm.console_indent);
    let text = if pad.is_empty() {
        text.to_string()
    } else {
        text.lines().map(|l| format!("{pad}{l}")).collect::<Vec<_>>().join("\n")
    };
    (vm.output)(kind, &text);
}

