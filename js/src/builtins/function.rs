//! Function e Function.prototype (call, apply, bind, toString).

use std::rc::Rc;

use super::Realm;
use crate::ops::data_in_chain;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let proto = &r.function_proto;
    {
        let mut b = proto.borrow_mut();
        b.props.insert(PropKey::str("length"), crate::ops::readonly(Value::Number(0.0)));
        b.props.insert(PropKey::str("name"), crate::ops::readonly(Value::str("")));
    }
    r.ctor("Function", 1, function_ctor, proto);
    // AsyncFunction, GeneratorFunction e AsyncGeneratorFunction: não são globais,
    // mas aparecem como `.constructor` das funções desses tipos
    for (fp, name, tag) in [
        (&r.async_function_proto, "AsyncFunction", "AsyncFunction"),
        (&r.generator_function_proto, "GeneratorFunction", "GeneratorFunction"),
        (&r.async_generator_function_proto, "AsyncGeneratorFunction", "AsyncGeneratorFunction"),
    ] {
        let c = super::native_fn(&r.function_proto, name, 1, function_ctor, true);
        super::set_native_data(&c, Value::str(name));
        c.borrow_mut().props.insert(
            PropKey::str("prototype"),
            Property {
                slot: Slot::Data {
                    value: Value::Object(fp.clone()),
                    writable: false,
                },
                enumerable: false,
                configurable: false,
            },
        );
        fp.borrow_mut().props.insert(PropKey::str("constructor"), Property::hidden(Value::Object(c)));
        r.tag(fp, tag);
    }
    r.generator_function_proto.borrow_mut().props.insert(
        PropKey::str("prototype"),
        Property::hidden(Value::Object(r.generator_proto.clone())),
    );
    r.generator_proto.borrow_mut().props.insert(
        PropKey::str("constructor"),
        Property::hidden(Value::Object(r.generator_function_proto.clone())),
    );
    r.method(proto, "call", 1, call);
    r.method(proto, "apply", 2, apply);
    r.method(proto, "bind", 1, bind);
    r.method(proto, "toString", 0, to_string);
    proto.borrow_mut().props.insert(
        PropKey::Sym(r.sym_has_instance.clone()),
        Property {
            slot: Slot::Data {
                value: Value::Object(r.function_has_instance.clone()),
                writable: false,
            },
            enumerable: false,
            configurable: false,
        },
    );
}

