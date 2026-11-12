//! Object e Object.prototype.

use super::{Realm, arg_str, this_obj};
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Vm, same_value, truthy};

pub fn install(r: &Realm) {
    let proto = &r.object_proto;
    let c = r.ctor("Object", 1, object_ctor, proto);
    r.method(&c, "keys", 1, keys);
    r.method(&c, "values", 1, values);
    r.method(&c, "entries", 1, entries);
    r.method(&c, "assign", 2, assign);
    r.method(&c, "create", 2, create);
    r.method(&c, "defineProperty", 3, define_property);
    r.method(&c, "defineProperties", 2, define_properties);
    r.method(&c, "getOwnPropertyNames", 1, get_own_property_names);
    r.method(&c, "getOwnPropertySymbols", 1, get_own_property_symbols);
    r.method(&c, "getOwnPropertyDescriptor", 2, get_own_property_descriptor);
    r.method(&c, "getOwnPropertyDescriptors", 1, get_own_property_descriptors);
    r.method(&c, "getPrototypeOf", 1, get_prototype_of);
    r.method(&c, "setPrototypeOf", 2, set_prototype_of);
    r.method(&c, "freeze", 1, freeze);
    r.method(&c, "isFrozen", 1, is_frozen);
    r.method(&c, "seal", 1, seal);
    r.method(&c, "isSealed", 1, is_sealed);
    r.method(&c, "preventExtensions", 1, prevent_extensions);
    r.method(&c, "isExtensible", 1, is_extensible);
    r.method(&c, "is", 2, is);
    r.method(&c, "fromEntries", 1, from_entries);
    r.method(&c, "hasOwn", 2, has_own);
    r.method(&c, "groupBy", 2, group_by);

    r.method(proto, "hasOwnProperty", 1, has_own_property);
    r.method(proto, "isPrototypeOf", 1, is_prototype_of);
    r.method(proto, "propertyIsEnumerable", 1, property_is_enumerable);
    r.method(proto, "toString", 0, to_string);
    r.method(proto, "toLocaleString", 0, to_locale_string);
    r.method(proto, "valueOf", 0, value_of);
    r.method(proto, "__defineGetter__", 2, define_getter);
    r.method(proto, "__defineSetter__", 2, define_setter);
    r.method(proto, "__lookupGetter__", 1, lookup_getter);
    let get = r.native("get __proto__", 0, proto_get);
    let set = r.native("set __proto__", 1, proto_set);
    proto.borrow_mut().props.insert(
        PropKey::str("__proto__"),
        Property {
            slot: Slot::Accessor {
                get: Some(get),
                set: Some(set),
            },
            enumerable: false,
            configurable: true,
        },
    );
}

