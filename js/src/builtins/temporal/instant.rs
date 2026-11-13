//! Temporal.Instant: um ponto exato no tempo (ns desde 1970-01-01T00:00Z).

use super::duration;
use super::iso::{self, NS_PER_DAY};
use super::options::{self, Group, UnitOpt};
use super::parse::{self, Goal};
use super::round::{self, RoundingMode, Unit};
use super::zone::{self, Tz};
use super::{Cal, TemporalData, bigint_from_i128, bigint_to_i128, class, create_instant, create_zoned, range, this_instant, type_err};
use crate::builtins::Realm;
use crate::value::*;
use crate::vm::{Hint, Vm};

pub fn install(r: &Realm, ns: &ObjRef, p: &ObjRef) {
    let c = class(r, ns, "Instant", 1, ctor, p);
    r.method(&c, "from", 1, from);
    r.method(&c, "fromEpochMilliseconds", 1, from_epoch_ms);
    r.method(&c, "fromEpochNanoseconds", 1, from_epoch_ns);
    r.method(&c, "compare", 2, compare);
    r.getter(p, "epochMilliseconds", epoch_ms);
    r.getter(p, "epochNanoseconds", epoch_ns);
    r.method(p, "add", 1, add);
    r.method(p, "subtract", 1, subtract);
    r.method(p, "until", 1, until);
    r.method(p, "since", 1, since);
    r.method(p, "round", 1, round);
    r.method(p, "equals", 1, equals);
    r.method(p, "toString", 0, to_string);
    r.method(p, "toLocaleString", 0, to_locale_string);
    r.method(p, "toJSON", 0, to_json);
    r.method(p, "valueOf", 0, value_of);
    r.method(p, "toZonedDateTimeISO", 1, to_zoned_date_time_iso);
}

/// ToBigInt → ns (RangeError fora do intervalo do i128).
fn to_ns(vm: &mut Vm, v: &Value) -> Result<i128, Value> {
    let b = super::super::bigint::to_bigint(vm, v)?;
    match bigint_to_i128(&b) {
        Some(n) => Ok(n),
        None => range(vm, "Invalid epoch nanoseconds: outside of supported range"),
    }
}

/// ToTemporalInstant → ns.
pub fn to_instant(vm: &mut Vm, v: &Value) -> Result<i128, Value> {
    let mut item = v.clone();
    if let Value::Object(_) = v {
        match super::data(v) {
            Some(TemporalData::Instant(n)) | Some(TemporalData::Zoned(n, _, _)) => return Ok(n),
            _ => item = vm.to_primitive(v, Hint::String)?,
        }
    }
    let Value::String(s) = &item else {
        let d = vm.describe(&item);
        return type_err(vm, format!("Invalid instant: {d} is not a string"));
    };
    let s = s.to_rust();
    let p = match parse::parse_iso(&s, &[Goal::Instant]) {
        Ok(p) => p,
        Err(e) => return range(vm, e),
    };
    let offset = if p.z { 0 } else { parse::parse_utc_offset(p.offset.as_deref().unwrap_or("")).unwrap_or(0) };
    let dt = iso::IsoDate::new(p.year.unwrap_or(0), p.month, p.day).at(p.time.unwrap_or_default());
    let b = iso::add_ns_to_date_time(&dt, -offset);
    if !iso::iso_days_in_range(&b.date) {
        return range(vm, format!("Instant is outside of supported range: {s}"));
    }
    let ns = iso::epoch_ns(&b);
    if !iso::is_valid_epoch_ns(ns) {
        return range(vm, format!("Instant is outside of supported range: {s}"));
    }
    Ok(ns)
}

/// TemporalInstantToString.
pub fn instant_string(vm: &mut Vm, ns: i128, tz: Option<&Tz>, precision: Option<u8>) -> String {
    let utc = Tz::utc();
    let out = tz.unwrap_or(&utc);
    let dt = zone::date_time_for(vm, out, ns);
    let mut s = iso::date_time_string(&dt, precision);
    match tz {
        None => s.push('Z'),
        Some(tz) => s.push_str(&zone::format_offset_rounded(zone::offset_ns(vm, tz, ns))),
    }
    s
}

fn ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return type_err(vm, "Constructor Temporal.Instant requires 'new'");
    };
    let ns = to_ns(vm, &a.arg(0))?;
    create_instant(vm, ns, Some(&nt))
}

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let ns = to_instant(vm, &a.arg(0))?;
    create_instant(vm, ns, None)
}

fn from_epoch_ms(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let n = vm.to_number(&a.arg(0))?;
    if !n.is_finite() || n.trunc() != n {
        return range(vm, format!("The number {} cannot be converted to a BigInt because it is not an integer", crate::conv::number_to_string(n)));
    }
    if n.abs() > 8.64e15 {
        return range(vm, "Invalid epoch milliseconds: outside of supported range");
    }
    create_instant(vm, n as i128 * 1_000_000, None)
}

fn from_epoch_ns(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let ns = to_ns(vm, &a.arg(0))?;
    create_instant(vm, ns, None)
}

fn compare(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let one = to_instant(vm, &a.arg(0))?;
    let two = to_instant(vm, &a.arg(1))?;
    Ok(Value::Number(iso::ord(one.cmp(&two)) as f64))
}

fn epoch_ms(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let ns = this_instant(vm, a, "Temporal.Instant.prototype.epochMilliseconds")?;
    Ok(Value::Number(ns.div_euclid(1_000_000) as f64))
}

fn epoch_ns(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let ns = this_instant(vm, a, "Temporal.Instant.prototype.epochNanoseconds")?;
    Ok(bigint_from_i128(ns))
}

/// AddDurationToInstant.
fn add_duration(vm: &mut Vm, a: &CallArgs, sub: bool, method: &str) -> Result<Value, Value> {
    let ns = this_instant(vm, a, method)?;
    let mut f = duration::to_duration(vm, &a.arg(0))?;
    if sub {
        f = duration::negate(&f);
    }
    let largest = duration::default_largest(&f);
    if largest.is_date() {
        return range(vm, format!("Duration field {} not supported by Temporal.Instant. Try Temporal.ZonedDateTime instead.", largest.singular()));
    }
    let r = round::add_instant(vm, ns, duration::to_internal_24h(&f).time)?;
    create_instant(vm, r, None)
}

fn add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, false, "Temporal.Instant.prototype.add")
}

fn subtract(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, true, "Temporal.Instant.prototype.subtract")
}

/// Configuração de until/since (GetDifferenceSettings).
pub struct Settings {
    pub largest: Unit,
    pub smallest: Unit,
    pub inc: i64,
    pub mode: RoundingMode,
}

/// GetDifferenceSettings.
pub fn difference_settings(
    vm: &mut Vm,
    since: bool,
    o: &options::Opts,
    group: Group,
    disallowed: &[Unit],
    fallback_smallest: Unit,
    default_largest: Unit,
) -> Result<Settings, Value> {
    let largest = options::unit(vm, o, "largestUnit", false)?;
    let inc = options::rounding_increment(vm, o)?;
    let mut mode = options::rounding_mode(vm, o, RoundingMode::Trunc)?;
    let smallest = options::unit(vm, o, "smallestUnit", false)?;
    options::validate_unit(vm, largest, group, &[UnitOpt::Auto])?;
    if let UnitOpt::U(u) = largest
        && disallowed.contains(&u)
    {
        return range(vm, format!("{} is not a valid value for largestUnit", u.singular()));
    }
    options::validate_unit(vm, smallest, group, &[])?;
    let smallest = match smallest {
        UnitOpt::U(u) => u,
        _ => fallback_smallest,
    };
    if disallowed.contains(&smallest) {
        return range(vm, format!("{} is not a valid value for smallestUnit", smallest.singular()));
    }
    let def = round::larger(default_largest, smallest);
    let largest = match largest {
        UnitOpt::U(u) => u,
        _ => def,
    };
    if round::larger(largest, smallest) != largest {
        return range(vm, format!("largestUnit {} cannot be smaller than smallestUnit {}", largest.singular(), smallest.singular()));
    }
    if let Some(max) = smallest.max_increment() {
        options::validate_increment(vm, inc, max, false)?;
    }
    if since {
        mode = mode.negate();
    }
    Ok(Settings { largest, smallest, inc, mode })
}

/// Cria a Duration de uma diferença (negada em `since`).
pub fn difference_result(vm: &mut Vm, d: &round::InternalDur, largest: Unit, since: bool) -> Result<Value, Value> {
    let mut f = duration::from_internal(d, largest);
    if !duration::is_valid(&f) {
        return range(vm, "Invalid duration");
    }
    if since {
        f = duration::negate(&f);
    }
    super::create_duration(vm, f, None)
}

fn difference(vm: &mut Vm, a: &CallArgs, since: bool, method: &str) -> Result<Value, Value> {
    let ns = this_instant(vm, a, method)?;
    let other = to_instant(vm, &a.arg(0))?;
    let o = options::get_options(vm, &a.arg(1))?;
    let s = difference_settings(vm, since, &o, Group::Time, &[], Unit::Nanosecond, Unit::Second)?;
    let d = round::diff_epoch(vm, ns, other, s.inc, s.smallest, s.mode)?;
    difference_result(vm, &d, s.largest, since)
}

fn until(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, false, "Temporal.Instant.prototype.until")
}

fn since(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, true, "Temporal.Instant.prototype.since")
}

/// Opções de round: string vira { smallestUnit }.
pub fn round_options(vm: &mut Vm, v: &Value) -> Result<options::Opts, Value> {
    if v.is_undefined() {
        return type_err(vm, "options parameter is required");
    }
    if let Value::String(_) = v {
        return Ok(Some(super::options_with("smallestUnit", v.clone())));
    }
    options::get_options(vm, v)
}

fn round(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let ns = this_instant(vm, a, "Temporal.Instant.prototype.round")?;
    let o = round_options(vm, &a.arg(0))?;
    let inc = options::rounding_increment(vm, &o)?;
    let mode = options::rounding_mode(vm, &o, RoundingMode::HalfExpand)?;
    let smallest = options::unit(vm, &o, "smallestUnit", true)?;
    options::validate_unit(vm, smallest, Group::Time, &[])?;
    let UnitOpt::U(unit) = smallest else {
        return range(vm, "smallestUnit option is required");
    };
    let max = (NS_PER_DAY / unit.ns()) as i64;
    options::validate_increment(vm, inc, max, true)?;
    let r = round::round_positive(ns, unit.ns() * inc as i128, mode);
    create_instant(vm, r, None)
}

fn equals(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let ns = this_instant(vm, a, "Temporal.Instant.prototype.equals")?;
    let other = to_instant(vm, &a.arg(0))?;
    Ok(Value::Bool(ns == other))
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let ns = this_instant(vm, a, "Temporal.Instant.prototype.toString")?;
    let o = options::get_options(vm, &a.arg(0))?;
    let digits = options::fractional_digits(vm, &o)?;
    let mode = options::rounding_mode(vm, &o, RoundingMode::Trunc)?;
    let smallest = options::unit(vm, &o, "smallestUnit", false)?;
    let tz_v = options::get(vm, &o, "timeZone")?;
    options::validate_unit(vm, smallest, Group::Time, &[])?;
    if smallest == UnitOpt::U(Unit::Hour) {
        return range(vm, "smallestUnit must be a time unit other than \"hour\"");
    }
    let tz = if tz_v.is_undefined() { None } else { Some(zone::to_time_zone(vm, &tz_v)?) };
    let (precision, unit, inc) = options::seconds_precision(smallest, digits);
    let r = round::round_positive(ns, unit.ns() * inc as i128, mode);
    if !iso::is_valid_epoch_ns(r) {
        return range(vm, "Instant is outside of supported range");
    }
    Ok(Value::str(&instant_string(vm, r, tz.as_ref(), precision)))
}

fn to_json(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let ns = this_instant(vm, a, "Temporal.Instant.prototype.toJSON")?;
    Ok(Value::str(&instant_string(vm, ns, None, None)))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    this_instant(vm, a, "Temporal.Instant.prototype.toLocaleString")?;
    let s = super::super::intl::temporal_to_locale(vm, &a.this, &a.arg(0), &a.arg(1), "any", "all", None)?;
    Ok(Value::str(&s))
}

fn value_of(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    super::value_of_throws(vm, "Instant")
}

fn to_zoned_date_time_iso(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let ns = this_instant(vm, a, "Temporal.Instant.prototype.toZonedDateTimeISO")?;
    let tz = zone::to_time_zone(vm, &a.arg(0))?;
    create_zoned(vm, ns, tz, Cal::Iso, None)
}

/// Date.prototype.toTemporalInstant.
pub fn date_to_temporal_instant(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let t = match &a.this {
        Value::Object(o) => match o.borrow().kind {
            Kind::Date(t) => Some(t),
            _ => None,
        },
        _ => None,
    };
    let Some(t) = t else {
        return type_err(vm, "this is not a Date object.");
    };
    if t.is_nan() {
        return range(vm, "Invalid time value");
    }
    create_instant(vm, t as i128 * 1_000_000, None)
}
