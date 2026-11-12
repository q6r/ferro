//! RegExp: a API do JavaScript sobre o motor de `crate::regex`.

use std::rc::Rc;

use super::array::iter_result;
use super::strings::get_substitution;
use super::{Realm, native_fn};
use crate::regex::{Flags, NONE, Program};
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Vm, same_value, truthy};

pub fn install(r: &Realm) {
    let p = &r.regexp_proto;
    let c = r.ctor("RegExp", 2, regexp_ctor, p);
    r.getter_sym(&c, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.method(&c, "escape", 1, escape);
    r.getter(p, "source", source);
    r.getter(p, "flags", flags);
    for (name, f) in [
        ("global", flag_g as NativeFn),
        ("ignoreCase", flag_i),
        ("multiline", flag_m),
        ("dotAll", flag_s),
        ("unicode", flag_u),
        ("unicodeSets", flag_v),
        ("sticky", flag_y),
        ("hasIndices", flag_d),
    ] {
        r.getter(p, name, f);
    }
    r.method(p, "toString", 0, to_string);
    p.borrow_mut().props.insert(PropKey::str("exec"), Property::hidden(Value::Object(r.regexp_exec.clone())));
    r.method(p, "test", 1, test);
    r.method(p, "compile", 2, compile_legacy);
    r.method_sym(p, &r.sym_match, "[Symbol.match]", 1, sym_match);
    r.method_sym(p, &r.sym_match_all, "[Symbol.matchAll]", 1, sym_match_all);
    r.method_sym(p, &r.sym_replace, "[Symbol.replace]", 2, sym_replace);
    r.method_sym(p, &r.sym_search, "[Symbol.search]", 1, sym_search);
    r.method_sym(p, &r.sym_split, "[Symbol.split]", 2, sym_split);
    let ip = &r.regexp_string_iter_proto;
    r.method(ip, "next", 0, string_iter_next);
    r.tag(ip, "RegExp String Iterator");
}

pub fn exec_native(function_proto: &ObjRef) -> ObjRef {
    native_fn(function_proto, "exec", 1, exec, false)
}

impl Vm {
    /// Compila (com cache) um padrão; erros viram SyntaxError como no V8.
    pub fn compile_regex(&mut self, source: &JsStr, flags: &JsStr) -> Result<Rc<Program>, Value> {
        let key = (source.clone(), flags.clone());
        if let Some(p) = self.regex_cache.get(&key) {
            return Ok(p.clone());
        }
        let f = flags.to_rust();
        let Some(fl) = Flags::parse(&f) else {
            return Err(self.syntax_error(format!("Invalid flags supplied to RegExp constructor '{f}'")));
        };
        match crate::regex::compile(source.units(), fl) {
            Ok(p) => {
                let p = Rc::new(p);
                if self.regex_cache.len() > 2000 {
                    self.regex_cache.clear();
                }
                self.regex_cache.insert(key, p.clone());
                Ok(p)
            }
            Err(e) => Err(self.syntax_error(format!("Invalid regular expression: /{}/{}: {}", source.to_rust(), f, e.0))),
        }
    }
}

