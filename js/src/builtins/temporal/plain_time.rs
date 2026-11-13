//! Temporal.PlainTime: hora do relógio, sem data nem fuso.

use super::duration;
use super::instant::{difference_result, difference_settings, round_options};
use super::iso::{self, IsoDate, Time};
use super::options::{self, Group, UnitOpt};
use super::parse::{self, Goal};
use super::round::{self, InternalDur, RoundingMode, Unit};
use super::zone;
use super::{TemporalData, check_partial, class, create_time, locale_string, range, sat, this_time, to_int_trunc, type_err};
use crate::builtins::Realm;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm, ns: &ObjRef, p: &ObjRef) {
    let c = class(r, ns, "PlainTime", 0, ctor, p);
    r.method(&c, "from", 1, from);
    r.method(&c, "compare", 2, compare);
    let getters: [(&str, NativeFn); 6] = [
        ("hour", get_hour),
        ("minute", get_minute),
        ("second", get_second),
        ("millisecond", get_ms),
        ("microsecond", get_us),
        ("nanosecond", get_ns),
    ];
    for (name, f) in getters {
        r.getter(p, name, f);
    }
    r.method(p, "add", 1, add);
    r.method(p, "subtract", 1, subtract);
    r.method(p, "with", 1, with);
    r.method(p, "until", 1, until);
    r.method(p, "since", 1, since);
    r.method(p, "round", 1, round);
    r.method(p, "equals", 1, equals);
    r.method(p, "toString", 0, to_string);
    r.method(p, "toLocaleString", 0, to_locale_string);
    r.method(p, "toJSON", 0, to_json);
    r.method(p, "valueOf", 0, value_of);
}

const TIME_NAMES: [&str; 6] = ["hour", "minute", "second", "millisecond", "microsecond", "nanosecond"];

/// ToTemporalTimeRecord: campos na ordem alfabética das propriedades.
pub fn time_record(vm: &mut Vm, bag: &Value, partial: bool) -> Result<[Option<f64>; 6], Value> {
    // hour, microsecond, millisecond, minute, nanosecond, second
    const ORDER: [usize; 6] = [0, 4, 3, 1, 5, 2];
    let mut out = [None; 6];
    let mut any = false;
    for k in ORDER {
        let v = vm.get_str(bag, TIME_NAMES[k])?;
        if !v.is_undefined() {
            out[k] = Some(to_int_trunc(vm, &v, TIME_NAMES[k])?);
            any = true;
        }
    }
    if !any {
        return type_err(vm, "Invalid time-like object: no time properties");
    }
    if !partial {
        for x in out.iter_mut() {
            x.get_or_insert(0.0);
        }
    }
    Ok(out)
}

pub fn regulate(vm: &mut Vm, f: &[f64; 6], constrain: bool) -> Result<Time, Value> {
    match iso::regulate_time(sat(f[0]), sat(f[1]), sat(f[2]), sat(f[3]), sat(f[4]), sat(f[5]), constrain) {
        Some(t) => Ok(t),
        None => range(vm, "Invalid time: value out of range"),
    }
}

/// ToTemporalTime.
pub fn to_time(vm: &mut Vm, v: &Value, options: &Value) -> Result<Time, Value> {
    if let Value::Object(_) = v {
        let direct = match super::data(v) {
            Some(TemporalData::Time(t)) => Some(t),
            Some(TemporalData::DateTime(dt, _)) => Some(dt.time),
            Some(TemporalData::Zoned(ns, tz, _)) => Some(zone::date_time_for(vm, &tz, ns).time),
            _ => None,
        };
        if let Some(t) = direct {
            let o = options::get_options(vm, options)?;
            options::overflow(vm, &o)?;
            return Ok(t);
        }
        let rec = time_record(vm, v, false)?;
        let o = options::get_options(vm, options)?;
        let constrain = options::overflow(vm, &o)?;
        return regulate(vm, &rec.map(|x| x.unwrap_or(0.0)), constrain);
    }
    let Value::String(s) = v else {
        let d = vm.describe(v);
        return type_err(vm, format!("Invalid time: {d}"));
    };
    let s = s.to_rust();
    let p = match parse::parse_iso(&s, &[Goal::Time]) {
        Ok(p) => p,
        Err(e) => return range(vm, e),
    };
    let o = options::get_options(vm, options)?;
    options::overflow(vm, &o)?;
    Ok(p.time.unwrap_or_default())
}

/// ToTimeRecordOrMidnight.
pub fn to_time_or_midnight(vm: &mut Vm, v: &Value) -> Result<Time, Value> {
    if v.is_undefined() {
        return Ok(iso::MIDNIGHT);
    }
    to_time(vm, v, &Value::Undefined)
}

fn ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return type_err(vm, "Constructor Temporal.PlainTime requires 'new'");
    };
    let mut f = [0.0; 6];
    for (k, x) in f.iter_mut().enumerate() {
        let v = a.arg(k);
        if !v.is_undefined() {
            *x = to_int_trunc(vm, &v, TIME_NAMES[k])?;
        }
    }
    let t = regulate(vm, &f, false)?;
    create_time(vm, t, Some(&nt))
}

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let t = to_time(vm, &a.arg(0), &a.arg(1))?;
    create_time(vm, t, None)
}

fn compare(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let one = to_time(vm, &a.arg(0), &Value::Undefined)?;
    let two = to_time(vm, &a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(iso::ord(one.cmp(&two)) as f64))
}

macro_rules! getter {
    ($name:ident, $f:ident, $js:expr) => {
        fn $name(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
            let t = this_time(vm, a, concat!("Temporal.PlainTime.prototype.", $js))?;
            Ok(Value::Number(t.$f as f64))
        }
    };
}

getter!(get_hour, hour, "hour");
getter!(get_minute, minute, "minute");
getter!(get_second, second, "second");
getter!(get_ms, ms, "millisecond");
getter!(get_us, us, "microsecond");
getter!(get_ns, ns, "nanosecond");

/// AddDurationToTime.
fn add_duration(vm: &mut Vm, a: &CallArgs, sub: bool, method: &str) -> Result<Value, Value> {
    let t = this_time(vm, a, method)?;
    let mut f = duration::to_duration(vm, &a.arg(0))?;
    if sub {
        f = duration::negate(&f);
    }
    let (_, r) = iso::add_time(&t, duration::to_internal_24h(&f).time);
    create_time(vm, r, None)
}

fn add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, false, "Temporal.PlainTime.prototype.add")
}

fn subtract(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, true, "Temporal.PlainTime.prototype.subtract")
}

fn with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let t = this_time(vm, a, "Temporal.PlainTime.prototype.with")?;
    let like = a.arg(0);
    check_partial(vm, &like)?;
    let p = time_record(vm, &like, true)?;
    let cur = [t.hour as f64, t.minute as f64, t.second as f64, t.ms as f64, t.us as f64, t.ns as f64];
    let mut f = cur;
    for k in 0..6 {
        if let Some(v) = p[k] {
            f[k] = v;
        }
    }
    let o = options::get_options(vm, &a.arg(1))?;
    let constrain = options::overflow(vm, &o)?;
    let r = regulate(vm, &f, constrain)?;
    create_time(vm, r, None)
}

fn difference(vm: &mut Vm, a: &CallArgs, since: bool, method: &str) -> Result<Value, Value> {
    let t = this_time(vm, a, method)?;
    let other = to_time(vm, &a.arg(0), &Value::Undefined)?;
    let o = options::get_options(vm, &a.arg(1))?;
    let s = difference_settings(vm, since, &o, Group::Time, &[], Unit::Nanosecond, Unit::Hour)?;
    let td = round::round_time_dur(vm, round::diff_time(&t, &other), s.inc, s.smallest, s.mode)?;
    difference_result(vm, &InternalDur { date: Default::default(), time: td }, s.largest, since)
}

fn until(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, false, "Temporal.PlainTime.prototype.until")
}

fn since(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, true, "Temporal.PlainTime.prototype.since")
}

fn round(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let t = this_time(vm, a, "Temporal.PlainTime.prototype.round")?;
    let o = round_options(vm, &a.arg(0))?;
    let inc = options::rounding_increment(vm, &o)?;
    let mode = options::rounding_mode(vm, &o, RoundingMode::HalfExpand)?;
    let smallest = options::unit(vm, &o, "smallestUnit", true)?;
    options::validate_unit(vm, smallest, Group::Time, &[])?;
    let UnitOpt::U(unit) = smallest else {
        return range(vm, "smallestUnit option is required");
    };
    options::validate_increment(vm, inc, unit.max_increment().unwrap_or(1000), false)?;
    let (_, r) = round::round_time(&t, inc, unit, mode);
    create_time(vm, r, None)
}

fn equals(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let t = this_time(vm, a, "Temporal.PlainTime.prototype.equals")?;
    let other = to_time(vm, &a.arg(0), &Value::Undefined)?;
    Ok(Value::Bool(t == other))
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let t = this_time(vm, a, "Temporal.PlainTime.prototype.toString")?;
    let o = options::get_options(vm, &a.arg(0))?;
    let digits = options::fractional_digits(vm, &o)?;
    let mode = options::rounding_mode(vm, &o, RoundingMode::Trunc)?;
    let smallest = options::unit(vm, &o, "smallestUnit", false)?;
    options::validate_unit(vm, smallest, Group::Time, &[])?;
    if smallest == UnitOpt::U(Unit::Hour) {
        return range(vm, "smallestUnit must be a time unit other than \"hour\"");
    }
    let (precision, unit, inc) = options::seconds_precision(smallest, digits);
    let (_, r) = round::round_time(&t, inc, unit, mode);
    Ok(Value::str(&iso::time_string(&r, precision)))
}

fn to_json(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let t = this_time(vm, a, "Temporal.PlainTime.prototype.toJSON")?;
    Ok(Value::str(&iso::time_string(&t, None)))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let t = this_time(vm, a, "Temporal.PlainTime.prototype.toLocaleString")?;
    locale_string(vm, &IsoDate::new(1970, 1, 1).at(t), &a.arg(0), &a.arg(1), "time", "time")
}

fn value_of(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    super::value_of_throws(vm, "PlainTime")
}
