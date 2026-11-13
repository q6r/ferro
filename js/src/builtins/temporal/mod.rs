//! Temporal (TC39): Instant, PlainDate, PlainTime, PlainDateTime,
//! PlainYearMonth, PlainMonthDay, ZonedDateTime, Duration e Now.
//!
//! Calendário ISO 8601. Os fusos com nome (IANA) vêm do hospedeiro
//! (`Vm::zones`); sem ele, só UTC e deslocamentos fixos. Instantes em ns
//! (i128), durações de tempo em ns (i128) e campos de Duration como Number.

mod cal_data;
pub mod calendar;
mod cals;
pub mod iso;
pub mod parse;
pub mod zone;

mod duration;
mod instant;
mod month_day;
mod noniso;
mod now;
mod options;
mod plain_date;
mod plain_date_time;
mod plain_time;
mod round;
mod year_month;
mod zoned;

use std::rc::Rc;

use super::Realm;
use crate::bigint::BigInt;
use crate::value::*;
use crate::vm::Vm;
pub use calendar::Cal;
use iso::{DateTime, IsoDate, Time};
pub use zone::Tz;

/// Dados internos de um objeto Temporal.
#[derive(Clone, Debug)]
pub enum TemporalData {
    /// anos, meses, semanas, dias, horas, minutos, segundos, ms, µs, ns.
    Duration([f64; 10]),
    Instant(i128),
    Date(IsoDate, Cal),
    Time(Time),
    DateTime(DateTime, Cal),
    YearMonth(IsoDate, Cal),
    MonthDay(IsoDate, Cal),
    Zoned(i128, Tz, Cal),
}

/// Protótipos das classes (criados com o Realm; preenchidos em `install`).
pub struct Protos {
    pub duration: ObjRef,
    pub instant: ObjRef,
    pub date: ObjRef,
    pub time: ObjRef,
    pub date_time: ObjRef,
    pub year_month: ObjRef,
    pub month_day: ObjRef,
    pub zoned: ObjRef,
}

impl Protos {
    pub fn new(object_proto: &ObjRef) -> Protos {
        let p = || ObjRef::new(Object::new(Some(object_proto.clone()), Kind::Ordinary));
        Protos {
            duration: p(),
            instant: p(),
            date: p(),
            time: p(),
            date_time: p(),
            year_month: p(),
            month_day: p(),
            zoned: p(),
        }
    }
}

pub fn install(r: &Realm) {
    let ns = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.tag(&ns, "Temporal");
    let p = &r.temporal;
    duration::install(r, &ns, &p.duration);
    instant::install(r, &ns, &p.instant);
    plain_date::install(r, &ns, &p.date);
    plain_time::install(r, &ns, &p.time);
    plain_date_time::install(r, &ns, &p.date_time);
    year_month::install(r, &ns, &p.year_month);
    month_day::install(r, &ns, &p.month_day);
    zoned::install(r, &ns, &p.zoned);
    now::install(r, &ns);
    r.method(&r.date_proto, "toTemporalInstant", 0, instant::date_to_temporal_instant);
    r.global_value("Temporal", Value::Object(ns));
}

/// Construtor de uma classe Temporal (propriedade do objeto `Temporal`).
fn class(r: &Realm, ns: &ObjRef, name: &str, len: u32, f: NativeFn, proto: &ObjRef) -> ObjRef {
    let c = super::native_fn(&r.function_proto, name, len, f, true);
    c.borrow_mut().props.insert(
        PropKey::str("prototype"),
        Property {
            slot: Slot::Data {
                value: Value::Object(proto.clone()),
                writable: false,
            },
            enumerable: false,
            configurable: false,
        },
    );
    proto.borrow_mut().props.insert(r.key_constructor.clone(), Property::hidden(Value::Object(c.clone())));
    ns.borrow_mut().props.insert(PropKey::str(name), Property::hidden(Value::Object(c.clone())));
    r.tag(proto, &format!("Temporal.{name}"));
    c
}

// ------------------------------------------------------------ erros

pub(crate) fn range<T>(vm: &mut Vm, msg: impl Into<String>) -> Result<T, Value> {
    Err(vm.range_error(msg))
}

pub(crate) fn type_err<T>(vm: &mut Vm, msg: impl Into<String>) -> Result<T, Value> {
    Err(vm.type_error(msg))
}

// ------------------------------------------------- dados dos objetos

/// Os dados Temporal de um valor (None se não for objeto Temporal).
pub fn data(v: &Value) -> Option<TemporalData> {
    let Value::Object(o) = v else { return None };
    match &o.borrow().kind {
        Kind::Temporal(t) => Some((**t).clone()),
        _ => None,
    }
}

fn incompatible<T>(vm: &mut Vm, a: &CallArgs, method: &str) -> Result<T, Value> {
    let d = vm.describe(&a.this);
    type_err(vm, format!("Method {method} called on incompatible receiver {d}"))
}

macro_rules! this_fn {
    ($name:ident, $pat:pat => $out:expr, $t:ty) => {
        pub(crate) fn $name(vm: &mut Vm, a: &CallArgs, method: &str) -> Result<$t, Value> {
            match data(&a.this) {
                Some($pat) => Ok($out),
                _ => incompatible(vm, a, method),
            }
        }
    };
}

this_fn!(this_duration, TemporalData::Duration(d) => d, [f64; 10]);
this_fn!(this_instant, TemporalData::Instant(n) => n, i128);
this_fn!(this_date, TemporalData::Date(d, c) => (d, c), (IsoDate, Cal));
this_fn!(this_time, TemporalData::Time(t) => t, Time);
this_fn!(this_date_time, TemporalData::DateTime(d, c) => (d, c), (DateTime, Cal));
this_fn!(this_year_month, TemporalData::YearMonth(d, c) => (d, c), (IsoDate, Cal));
this_fn!(this_month_day, TemporalData::MonthDay(d, c) => (d, c), (IsoDate, Cal));
this_fn!(this_zoned, TemporalData::Zoned(n, tz, c) => (n, tz, c), (i128, Tz, Cal));

/// Objeto Temporal novo com o protótipo dado.
fn make(proto: &ObjRef, d: TemporalData) -> Value {
    Value::Object(ObjRef::new(Object::new(Some(proto.clone()), Kind::Temporal(Box::new(d)))))
}

/// Protótipo vindo do `new.target` (subclasses) ou o padrão.
fn proto_for(vm: &mut Vm, nt: Option<&ObjRef>, default: &ObjRef) -> Result<ObjRef, Value> {
    match nt {
        Some(nt) => vm.proto_from_ctor(Some(nt), default),
        None => Ok(default.clone()),
    }
}

pub fn create_duration(vm: &mut Vm, f: [f64; 10], nt: Option<&ObjRef>) -> Result<Value, Value> {
    if !duration::is_valid(&f) {
        return range(vm, "Invalid duration");
    }
    let def = vm.realm.temporal.duration.clone();
    let p = proto_for(vm, nt, &def)?;
    Ok(make(&p, TemporalData::Duration(f.map(|x| x + 0.0))))
}

pub fn create_instant(vm: &mut Vm, ns: i128, nt: Option<&ObjRef>) -> Result<Value, Value> {
    if !iso::is_valid_epoch_ns(ns) {
        return range(vm, "Invalid epoch nanoseconds: outside of supported range");
    }
    let def = vm.realm.temporal.instant.clone();
    let p = proto_for(vm, nt, &def)?;
    Ok(make(&p, TemporalData::Instant(ns)))
}

pub fn create_date(vm: &mut Vm, d: IsoDate, cal: Cal, nt: Option<&ObjRef>) -> Result<Value, Value> {
    if !iso::date_within_limits(&d) {
        return range(vm, "Date is outside of supported range");
    }
    let def = vm.realm.temporal.date.clone();
    let p = proto_for(vm, nt, &def)?;
    Ok(make(&p, TemporalData::Date(d, cal)))
}

pub fn create_time(vm: &mut Vm, t: Time, nt: Option<&ObjRef>) -> Result<Value, Value> {
    let def = vm.realm.temporal.time.clone();
    let p = proto_for(vm, nt, &def)?;
    Ok(make(&p, TemporalData::Time(t)))
}

pub fn create_date_time(vm: &mut Vm, dt: DateTime, cal: Cal, nt: Option<&ObjRef>) -> Result<Value, Value> {
    if !iso::date_time_within_limits(&dt) {
        return range(vm, "Date and time are outside of supported range");
    }
    let def = vm.realm.temporal.date_time.clone();
    let p = proto_for(vm, nt, &def)?;
    Ok(make(&p, TemporalData::DateTime(dt, cal)))
}

pub fn create_year_month(vm: &mut Vm, d: IsoDate, cal: Cal, nt: Option<&ObjRef>) -> Result<Value, Value> {
    if !iso::year_month_within_limits(d.year, d.month) {
        return range(vm, "Year and month are outside of supported range");
    }
    let def = vm.realm.temporal.year_month.clone();
    let p = proto_for(vm, nt, &def)?;
    Ok(make(&p, TemporalData::YearMonth(d, cal)))
}

pub fn create_month_day(vm: &mut Vm, d: IsoDate, cal: Cal, nt: Option<&ObjRef>) -> Result<Value, Value> {
    if !iso::date_within_limits(&d) {
        return range(vm, "Date is outside of supported range");
    }
    let def = vm.realm.temporal.month_day.clone();
    let p = proto_for(vm, nt, &def)?;
    Ok(make(&p, TemporalData::MonthDay(d, cal)))
}

pub fn create_zoned(vm: &mut Vm, ns: i128, tz: Tz, cal: Cal, nt: Option<&ObjRef>) -> Result<Value, Value> {
    if !iso::is_valid_epoch_ns(ns) {
        return range(vm, "Invalid epoch nanoseconds: outside of supported range");
    }
    let def = vm.realm.temporal.zoned.clone();
    let p = proto_for(vm, nt, &def)?;
    Ok(make(&p, TemporalData::Zoned(ns, tz, cal)))
}

// ---------------------------------------------------- conversões

/// ToIntegerWithTruncation.
pub fn to_int_trunc(vm: &mut Vm, v: &Value, what: &str) -> Result<f64, Value> {
    let n = vm.to_number(v)?;
    if !n.is_finite() {
        return range(vm, format!("Invalid {what}: {}", crate::conv::number_to_string(n)));
    }
    Ok(n.trunc() + 0.0)
}

/// ToPositiveIntegerWithTruncation.
pub fn to_positive_int(vm: &mut Vm, v: &Value, what: &str) -> Result<f64, Value> {
    let n = to_int_trunc(vm, v, what)?;
    if n <= 0.0 {
        return range(vm, format!("Invalid {what}: must be positive"));
    }
    Ok(n)
}

/// ToIntegerIfIntegral.
pub fn to_int_integral(vm: &mut Vm, v: &Value, what: &str) -> Result<f64, Value> {
    let n = vm.to_number(v)?;
    if !n.is_finite() || n.trunc() != n {
        return range(vm, format!("Invalid {what}: {} is not an integer", crate::conv::number_to_string(n)));
    }
    Ok(n + 0.0)
}

/// Número inteiro (Number) → i64 com saturação.
pub fn sat(x: f64) -> i64 {
    x as i64
}

/// i128 → BigInt.
pub fn bigint_from_i128(v: i128) -> Value {
    let m = v.unsigned_abs();
    let hi = BigInt::from_u64((m >> 64) as u64).shl(64);
    let b = hi.add(&BigInt::from_u64(m as u64));
    Value::BigInt(Rc::new(if v < 0 { b.neg() } else { b }))
}

/// BigInt → i128 (None se não couber).
pub fn bigint_to_i128(b: &BigInt) -> Option<i128> {
    b.to_string_radix(10).parse().ok()
}

/// Exige uma string (sem conversão).
pub fn require_string(vm: &mut Vm, v: &Value, what: &str) -> Result<String, Value> {
    match v {
        Value::String(s) => Ok(s.to_rust()),
        _ => {
            let d = vm.describe(v);
            type_err(vm, format!("Invalid {what}: {d} is not a string"))
        }
    }
}

/// Mensagem do valueOf dos objetos Temporal (comparação e aritmética).
fn value_of_throws(vm: &mut Vm, class: &str) -> Result<Value, Value> {
    let compare = if class == "PlainMonthDay" {
        "Temporal.PlainDate.compare(obj1.toPlainDate(year), obj2.toPlainDate(year))".to_string()
    } else {
        format!("Temporal.{class}.compare(obj1, obj2)")
    };
    type_err(vm, format!("Do not use built-in arithmetic operators with Temporal objects. When comparing, use {compare}, not obj1 > obj2."))
}

/// Cria um objeto de opções sem protótipo (round("hour") etc.).
fn options_with(key: &str, v: Value) -> ObjRef {
    let o = ObjRef::new(Object::new(None, Kind::Ordinary));
    o.borrow_mut().props.insert(PropKey::str(key), Property::data(v));
    o
}

/// IsPartialTemporalObject (TypeError quando não serve para `with`).
fn check_partial(vm: &mut Vm, v: &Value) -> Result<(), Value> {
    if !matches!(v, Value::Object(_)) {
        let d = vm.describe(v);
        return type_err(vm, format!("Invalid argument: {d} is not an object"));
    }
    if matches!(
        data(v),
        Some(
            TemporalData::Date(..)
                | TemporalData::DateTime(..)
                | TemporalData::MonthDay(..)
                | TemporalData::Time(..)
                | TemporalData::YearMonth(..)
                | TemporalData::Zoned(..)
        )
    ) {
        return type_err(vm, "Invalid argument: a Temporal object is not a valid partial object");
    }
    for key in ["calendar", "timeZone"] {
        if !vm.get_str(v, key)?.is_undefined() {
            return type_err(vm, format!("Invalid argument: {key} property is not supported here"));
        }
    }
    Ok(())
}

/// toLocaleString: Intl.DateTimeFormat com os `required`/`defaults` do tipo.
fn locale_string(vm: &mut Vm, a: &CallArgs, required: &str, defaults: &str) -> Result<Value, Value> {
    let s = super::intl::temporal_to_locale(vm, &a.this, &a.arg(0), &a.arg(1), required, defaults, None)?;
    Ok(Value::str(&s))
}

/// Getter com um índice nos dados da função (os campos de calendário).
fn data_getter(r: &Realm, target: &ObjRef, name: &str, f: NativeFn, idx: usize) {
    let fo = r.native(&format!("get {name}"), 0, f);
    super::set_native_data(&fo, Value::Number(idx as f64));
    target.borrow_mut().props.insert(
        PropKey::str(name),
        Property {
            slot: Slot::Accessor { get: Some(fo), set: None },
            enumerable: false,
            configurable: true,
        },
    );
}

/// Índice guardado por `data_getter`.
fn getter_index(a: &CallArgs) -> usize {
    match super::native_data(&a.callee) {
        Value::Number(n) => n as usize,
        _ => 0,
    }
}
