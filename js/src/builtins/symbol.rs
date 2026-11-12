//! Symbol.

use super::Realm;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let proto = &r.symbol_proto;
    let c = r.ctor("Symbol", 0, symbol_ctor, proto);
    // Symbol não é construtor de verdade (new Symbol lança), mas tem prototype
    if let Kind::Function(Func::Native(n)) = &mut c.borrow_mut().kind {
        n.ctor = false;
    }
    for (name, s) in [
        ("iterator", &r.sym_iterator),
        ("asyncIterator", &r.sym_async_iterator),
        ("hasInstance", &r.sym_has_instance),
        ("toPrimitive", &r.sym_to_primitive),
        ("toStringTag", &r.sym_to_string_tag),
        ("species", &r.sym_species),
        ("isConcatSpreadable", &r.sym_is_concat_spreadable),
        ("unscopables", &r.sym_unscopables),
        ("match", &r.sym_match),
        ("matchAll", &r.sym_match_all),
        ("replace", &r.sym_replace),
        ("search", &r.sym_search),
        ("split", &r.sym_split),
    ] {
        r.constant(&c, name, Value::Symbol(s.clone()));
    }
    r.method(&c, "for", 1, symbol_for);
    r.method(&c, "keyFor", 1, key_for);
    r.method(proto, "toString", 0, to_string);
    r.method(proto, "valueOf", 0, value_of);
    r.getter(proto, "description", description);
    r.method_sym(proto, &r.sym_to_primitive, "[Symbol.toPrimitive]", 1, value_of);
    r.tag(proto, "Symbol");
}

fn symbol_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if a.new_target.is_some() {
        return Err(vm.type_error("Symbol is not a constructor"));
    }
    let desc = if a.arg(0).is_undefined() { None } else { Some(vm.to_string(&a.arg(0))?) };
    Ok(Value::Symbol(Sym::new(desc)))
}

fn this_symbol(vm: &mut Vm, a: &CallArgs) -> Result<Sym, Value> {
    match &a.this {
        Value::Symbol(s) => Ok(s.clone()),
        Value::Object(o) => match &o.borrow().kind {
            Kind::Symbol(s) => Ok(s.clone()),
            _ => Err(vm.type_error("Symbol.prototype.valueOf requires that 'this' be a Symbol")),
        },
        _ => Err(vm.type_error("Symbol.prototype.valueOf requires that 'this' be a Symbol")),
    }
}

