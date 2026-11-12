//! Error e seus subtipos (TypeError, RangeError, ...).

use super::{Realm, native_data, set_native_data};
use crate::ops::data_in_chain;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let base = make(r, "Error", &r.error_proto, None);
    r.method(&r.error_proto, "toString", 0, to_string);
    r.method(&base, "captureStackTrace", 1, capture_stack_trace);
    base.borrow_mut().props.insert(PropKey::str("stackTraceLimit"), Property::data(Value::Number(10.0)));
    for (name, proto) in [
        ("TypeError", &r.type_error_proto),
        ("RangeError", &r.range_error_proto),
        ("ReferenceError", &r.reference_error_proto),
        ("SyntaxError", &r.syntax_error_proto),
        ("EvalError", &r.eval_error_proto),
        ("URIError", &r.uri_error_proto),
    ] {
        make(r, name, proto, Some(&base));
    }
    let agg_proto = ObjRef::new(Object::new(Some(r.error_proto.clone()), Kind::Ordinary));
    let agg = r.ctor("AggregateError", 2, aggregate_ctor, &agg_proto);
    agg.borrow_mut().proto = Some(base);
    set_native_data(&agg, Value::Object(agg_proto.clone()));
    agg_proto.borrow_mut().props.insert(PropKey::str("name"), Property::hidden(Value::str("AggregateError")));
    agg_proto.borrow_mut().props.insert(PropKey::str("message"), Property::hidden(Value::str("")));
}

fn make(r: &Realm, name: &str, proto: &ObjRef, parent: Option<&ObjRef>) -> ObjRef {
    let c = r.ctor(name, 1, error_ctor, proto);
    set_native_data(&c, Value::Object(proto.clone()));
    if let Some(p) = parent {
        c.borrow_mut().proto = Some(p.clone());
    }
    proto.borrow_mut().props.insert(PropKey::str("name"), Property::hidden(Value::str(name)));
    proto.borrow_mut().props.insert(PropKey::str("message"), Property::hidden(Value::str("")));
    c
}

