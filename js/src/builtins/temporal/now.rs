//! Temporal.Now: o instante atual e a data/hora no fuso do sistema.

use super::iso;
use super::zone::{self, Tz};
use super::{Cal, create_date, create_date_time, create_instant, create_time, create_zoned};
use crate::builtins::Realm;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm, ns: &ObjRef) {
    let now = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.tag(&now, "Temporal.Now");
    r.method(&now, "instant", 0, instant);
    r.method(&now, "timeZoneId", 0, time_zone_id);
    r.method(&now, "zonedDateTimeISO", 0, zoned_date_time_iso);
    r.method(&now, "plainDateTimeISO", 0, plain_date_time_iso);
    r.method(&now, "plainDateISO", 0, plain_date_iso);
    r.method(&now, "plainTimeISO", 0, plain_time_iso);
    ns.borrow_mut().props.insert(PropKey::str("Now"), Property::hidden(Value::Object(now)));
}

/// SystemUTCEpochNanoseconds.
fn system_ns() -> i128 {
    let ns = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    };
    ns.clamp(iso::NS_MIN, iso::NS_MAX)
}

/// Fuso do argumento (undefined: o do sistema).
fn tz_arg(vm: &mut Vm, v: &Value) -> Result<Tz, Value> {
    if v.is_undefined() {
        return Ok(zone::system(vm));
    }
    zone::to_time_zone(vm, v)
}

fn instant(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    create_instant(vm, system_ns(), None)
}

fn time_zone_id(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::str(&zone::system(vm).id()))
}

fn zoned_date_time_iso(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let tz = tz_arg(vm, &a.arg(0))?;
    create_zoned(vm, system_ns(), tz, Cal::Iso, None)
}

fn plain_date_time_iso(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let tz = tz_arg(vm, &a.arg(0))?;
    let dt = zone::date_time_for(vm, &tz, system_ns());
    create_date_time(vm, dt, Cal::Iso, None)
}

fn plain_date_iso(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let tz = tz_arg(vm, &a.arg(0))?;
    let dt = zone::date_time_for(vm, &tz, system_ns());
    create_date(vm, dt.date, Cal::Iso, None)
}

fn plain_time_iso(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let tz = tz_arg(vm, &a.arg(0))?;
    let dt = zone::date_time_for(vm, &tz, system_ns());
    create_time(vm, dt.time, None)
}
