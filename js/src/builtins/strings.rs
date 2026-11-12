//! String, String.prototype e o iterador de strings.

use super::{Realm, rel_index};
use super::array::iter_result;
use crate::string::{JsStr, is_js_space, trim_js};
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let proto = &r.string_proto;
    let c = r.ctor("String", 1, string_ctor, proto);
    r.method(&c, "fromCharCode", 1, from_char_code);
    r.method(&c, "fromCodePoint", 1, from_code_point);
    r.method(&c, "raw", 1, raw);

    r.method(proto, "at", 1, at);
    r.method(proto, "charAt", 1, char_at);
    r.method(proto, "charCodeAt", 1, char_code_at);
    r.method(proto, "codePointAt", 1, code_point_at);
    r.method(proto, "concat", 1, concat);
    r.method(proto, "endsWith", 1, ends_with);
    r.method(proto, "includes", 1, includes);
    r.method(proto, "indexOf", 1, index_of);
    r.method(proto, "lastIndexOf", 1, last_index_of);
    r.method(proto, "localeCompare", 1, locale_compare);
    r.method(proto, "normalize", 0, normalize);
    r.method(proto, "padEnd", 1, pad_end);
    r.method(proto, "padStart", 1, pad_start);
    r.method(proto, "repeat", 1, repeat);
    r.method(proto, "replace", 2, replace);
    r.method(proto, "replaceAll", 2, replace_all);
    r.method(proto, "slice", 2, slice);
    r.method(proto, "split", 2, split);
    r.method(proto, "startsWith", 1, starts_with);
    r.method(proto, "substring", 2, substring);
    r.method(proto, "substr", 2, substr);
    r.method(proto, "toLowerCase", 0, to_lower);
    r.method(proto, "toUpperCase", 0, to_upper);
    r.method(proto, "toLocaleLowerCase", 0, to_lower);
    r.method(proto, "toLocaleUpperCase", 0, to_upper);
    r.method(proto, "toString", 0, to_string);
    r.method(proto, "valueOf", 0, to_string);
    let trim_f = r.method(proto, "trim", 0, trim);
    let _ = trim_f;
    let ts = r.method(proto, "trimStart", 0, trim_start);
    let te = r.method(proto, "trimEnd", 0, trim_end);
    proto.borrow_mut().props.insert(PropKey::str("trimLeft"), Property::hidden(Value::Object(ts)));
    proto.borrow_mut().props.insert(PropKey::str("trimRight"), Property::hidden(Value::Object(te)));
    r.method(proto, "match", 1, match_);
    r.method(proto, "matchAll", 1, match_all);
    r.method(proto, "search", 1, search);
    r.method(proto, "isWellFormed", 0, is_well_formed);
    r.method(proto, "toWellFormed", 0, to_well_formed);
    r.method_sym(proto, &r.sym_iterator, "[Symbol.iterator]", 0, iterator);

    let ip = &r.string_iter_proto;
    r.method(ip, "next", 0, iter_next);
    r.tag(ip, "String Iterator");
}

