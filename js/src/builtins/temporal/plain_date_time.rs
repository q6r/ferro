//! Temporal.PlainDateTime: data e hora de calendário, sem fuso.

use super::calendar::{self, Cal, DATE_FIELDS, DATE_GETTERS, FieldsKind, Required, ShowCalendar, TIME_FIELDS};
use super::duration;
use super::instant::{difference_result, difference_settings, round_options};
use super::iso::{self, DateTime, IsoDate, MIDNIGHT, Time};
use super::options::{self, Group, UnitOpt};
use super::parse::{self, Goal};
use super::plain_date::ctor_calendar;
use super::plain_time::to_time_or_midnight;
use super::round::{self, RoundingMode, Unit};
use super::zone;
use super::{
    TemporalData, check_partial, class, create_date, create_date_time, create_time, create_zoned, data_getter, getter_index,
    incompatible, locale_string, range, sat, this_date_time, to_int_trunc, type_err,
};
use crate::builtins::Realm;
use crate::value::*;
use crate::vm::Vm;

const TIME_GETTERS: [&str; 6] = ["hour", "minute", "second", "millisecond", "microsecond", "nanosecond"];

pub fn install(r: &Realm, ns: &ObjRef, p: &ObjRef) {
    let c = class(r, ns, "PlainDateTime", 3, ctor, p);
    r.method(&c, "from", 1, from);
    r.method(&c, "compare", 2, compare);
    r.getter(p, "calendarId", calendar_id);
    for (k, (name, _)) in DATE_GETTERS.iter().enumerate() {
        data_getter(r, p, name, date_getter, k);
    }
    for (k, name) in TIME_GETTERS.iter().enumerate() {
        data_getter(r, p, name, time_getter, k);
    }
    r.method(p, "with", 1, with);
    r.method(p, "withPlainTime", 0, with_plain_time);
    r.method(p, "withCalendar", 1, with_calendar);
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
    r.method(p, "toZonedDateTime", 1, to_zoned_date_time);
    r.method(p, "toPlainDate", 0, to_plain_date);
    r.method(p, "toPlainTime", 0, to_plain_time);
}

/// Um campo de hora (hour = 0 ... nanosecond = 5).
pub fn time_field(t: &Time, k: usize) -> Value {
    Value::Number(match k {
        0 => t.hour as f64,
        1 => t.minute as f64,
        2 => t.second as f64,
        3 => t.ms as f64,
        4 => t.us as f64,
        _ => t.ns as f64,
    })
}

/// ToTemporalDateTime.
pub fn to_date_time(vm: &mut Vm, v: &Value, options: &Value) -> Result<(DateTime, Cal), Value> {
    if let Value::Object(_) = v {
        let direct = match super::data(v) {
            Some(TemporalData::DateTime(dt, c)) => Some((dt, c)),
            Some(TemporalData::Zoned(ns, tz, c)) => Some((zone::date_time_for(vm, &tz, ns), c)),
            Some(TemporalData::Date(d, c)) => Some((d.at(MIDNIGHT), c)),
            _ => None,
        };
        if let Some(r) = direct {
            let o = options::get_options(vm, options)?;
            options::overflow(vm, &o)?;
            return Ok(r);
        }
        let cal = calendar::calendar_with_iso_default(vm, v)?;
        let f = calendar::prepare(vm, cal, v, DATE_FIELDS | TIME_FIELDS, Required::None)?;
        let o = options::get_options(vm, options)?;
        let constrain = options::overflow(vm, &o)?;
        let dt = calendar::interpret_date_time(vm, cal, &f, constrain)?;
        if !iso::date_time_within_limits(&dt) {
            return range(vm, "Date and time are outside of supported range");
        }
        return Ok((dt, cal));
    }
    let Value::String(s) = v else {
        let d = vm.describe(v);
        return type_err(vm, format!("Invalid plain date-time: {d}"));
    };
    let s = s.to_rust();
    let p = match parse::parse_iso(&s, &[Goal::DateTime]) {
        Ok(p) => p,
        Err(e) => return range(vm, e),
    };
    let cal = calendar::canonicalize_or_throw(vm, p.calendar.as_deref().unwrap_or("iso8601"))?;
    let o = options::get_options(vm, options)?;
    options::overflow(vm, &o)?;
    let dt = IsoDate::new(p.year.unwrap_or(0), p.month, p.day).at(p.time.unwrap_or(MIDNIGHT));
    if !iso::date_time_within_limits(&dt) {
        return range(vm, format!("Date and time are outside of supported range: {s}"));
    }
    Ok((dt, cal))
}

fn ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return type_err(vm, "Constructor Temporal.PlainDateTime requires 'new'");
    };
    let y = to_int_trunc(vm, &a.arg(0), "year")?;
    let m = to_int_trunc(vm, &a.arg(1), "month")?;
    let d = to_int_trunc(vm, &a.arg(2), "day")?;
    let mut t = [0.0; 6];
    for (k, x) in t.iter_mut().enumerate() {
        let v = a.arg(3 + k);
        if !v.is_undefined() {
            *x = to_int_trunc(vm, &v, TIME_GETTERS[k])?;
        }
    }
    let cal = ctor_calendar(vm, &a.arg(9))?;
    let Some(date) = iso::regulate_date(sat(y), sat(m), sat(d), false) else {
        return range(vm, "Invalid ISO date");
    };
    let time = super::plain_time::regulate(vm, &t, false)?;
    create_date_time(vm, date.at(time), cal, Some(&nt))
}

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, cal) = to_date_time(vm, &a.arg(0), &a.arg(1))?;
    create_date_time(vm, dt, cal, None)
}

fn compare(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (one, _) = to_date_time(vm, &a.arg(0), &Value::Undefined)?;
    let (two, _) = to_date_time(vm, &a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(iso::cmp_date_time(&one, &two) as f64))
}

fn calendar_id(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, cal) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.calendarId")?;
    Ok(Value::str(cal.id()))
}

fn date_getter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (name, field) = DATE_GETTERS[getter_index(a)];
    match super::data(&a.this) {
        Some(TemporalData::DateTime(dt, c)) => Ok(calendar::date_field(&dt.date, c, field)),
        _ => incompatible(vm, a, &format!("Temporal.PlainDateTime.prototype.{name}")),
    }
}

fn time_getter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = getter_index(a);
    match super::data(&a.this) {
        Some(TemporalData::DateTime(dt, _)) => Ok(time_field(&dt.time, k)),
        _ => incompatible(vm, a, &format!("Temporal.PlainDateTime.prototype.{}", TIME_GETTERS[k])),
    }
}

/// Os campos de calendário de uma data e hora.
pub fn date_time_fields(cal: Cal, dt: &DateTime) -> calendar::Fields {
    let mut f = calendar::date_to_fields(cal, &dt.date, FieldsKind::Date);
    let t = &dt.time;
    f.hour = Some(t.hour as f64);
    f.minute = Some(t.minute as f64);
    f.second = Some(t.second as f64);
    f.millisecond = Some(t.ms as f64);
    f.microsecond = Some(t.us as f64);
    f.nanosecond = Some(t.ns as f64);
    f
}

fn with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.with")?;
    let like = a.arg(0);
    check_partial(vm, &like)?;
    let fields = date_time_fields(cal, &dt);
    let partial = calendar::prepare(vm, cal, &like, DATE_FIELDS | TIME_FIELDS, Required::Partial)?;
    let merged = calendar::merge(cal, &fields, &partial);
    let o = options::get_options(vm, &a.arg(1))?;
    let constrain = options::overflow(vm, &o)?;
    let r = calendar::interpret_date_time(vm, cal, &merged, constrain)?;
    create_date_time(vm, r, cal, None)
}

fn with_plain_time(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.withPlainTime")?;
    let t = to_time_or_midnight(vm, &a.arg(0))?;
    create_date_time(vm, dt.date.at(t), cal, None)
}

fn with_calendar(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, _) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.withCalendar")?;
    let cal = calendar::to_calendar(vm, &a.arg(0))?;
    create_date_time(vm, dt, cal, None)
}

/// AddDurationToDateTime.
fn add_duration(vm: &mut Vm, a: &CallArgs, sub: bool, method: &str) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, method)?;
    let mut f = duration::to_duration(vm, &a.arg(0))?;
    if sub {
        f = duration::negate(&f);
    }
    let o = options::get_options(vm, &a.arg(1))?;
    let constrain = options::overflow(vm, &o)?;
    let d = duration::to_internal_24h(&f);
    let (days, time) = iso::add_time(&dt.time, d.time);
    let dd = round::adjust(vm, &d.date, days, None, None)?;
    let date = calendar::date_add(vm, cal, &dt.date, &dd, constrain)?;
    create_date_time(vm, date.at(time), cal, None)
}

fn add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, false, "Temporal.PlainDateTime.prototype.add")
}

fn subtract(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, true, "Temporal.PlainDateTime.prototype.subtract")
}

fn difference(vm: &mut Vm, a: &CallArgs, since: bool, method: &str) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, method)?;
    let (other, ocal) = to_date_time(vm, &a.arg(0), &Value::Undefined)?;
    if cal != ocal {
        return range(vm, format!("Cannot compute difference between dates of {} and {} calendars", cal.id(), ocal.id()));
    }
    let o = options::get_options(vm, &a.arg(1))?;
    let s = difference_settings(vm, since, &o, Group::DateTime, &[], Unit::Nanosecond, Unit::Day)?;
    if dt == other {
        return super::create_duration(vm, [0.0; 10], None);
    }
    let r = round::diff_date_time_rounded(vm, &dt, &other, cal, s.largest, s.inc, s.smallest, s.mode)?;
    difference_result(vm, &r, s.largest, since)
}

fn until(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, false, "Temporal.PlainDateTime.prototype.until")
}

fn since(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, true, "Temporal.PlainDateTime.prototype.since")
}

/// Unidade e incremento de round() com dia permitido (PlainDateTime e ZonedDateTime).
pub fn round_settings(vm: &mut Vm, v: &Value) -> Result<(i64, RoundingMode, Unit), Value> {
    let o = round_options(vm, v)?;
    let inc = options::rounding_increment(vm, &o)?;
    let mode = options::rounding_mode(vm, &o, RoundingMode::HalfExpand)?;
    let smallest = options::unit(vm, &o, "smallestUnit", true)?;
    options::validate_unit(vm, smallest, Group::Time, &[UnitOpt::U(Unit::Day)])?;
    let UnitOpt::U(unit) = smallest else {
        return range(vm, "smallestUnit option is required");
    };
    match unit.max_increment() {
        Some(max) => options::validate_increment(vm, inc, max, false)?,
        None => options::validate_increment(vm, inc, 1, true)?,
    }
    Ok((inc, mode, unit))
}

fn round(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.round")?;
    let (inc, mode, unit) = round_settings(vm, &a.arg(0))?;
    if inc == 1 && unit == Unit::Nanosecond {
        return create_date_time(vm, dt, cal, None);
    }
    let r = round::round_date_time(&dt, inc, unit, mode);
    create_date_time(vm, r, cal, None)
}

fn equals(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.equals")?;
    let (other, ocal) = to_date_time(vm, &a.arg(0), &Value::Undefined)?;
    Ok(Value::Bool(dt == other && cal == ocal))
}

pub fn date_time_string(dt: &DateTime, cal: Cal, precision: Option<u8>, show: ShowCalendar) -> String {
    format!("{}{}", iso::date_time_string(dt, precision), calendar::annotation(cal, show))
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.toString")?;
    let o = options::get_options(vm, &a.arg(0))?;
    let show = options::show_calendar(vm, &o)?;
    let digits = options::fractional_digits(vm, &o)?;
    let mode = options::rounding_mode(vm, &o, RoundingMode::Trunc)?;
    let smallest = options::unit(vm, &o, "smallestUnit", false)?;
    options::validate_unit(vm, smallest, Group::Time, &[])?;
    if smallest == UnitOpt::U(Unit::Hour) {
        return range(vm, "smallestUnit must be a time unit other than \"hour\"");
    }
    let (precision, unit, inc) = options::seconds_precision(smallest, digits);
    let r = round::round_date_time(&dt, inc, unit, mode);
    if !iso::date_time_within_limits(&r) {
        return range(vm, "Date and time are outside of supported range");
    }
    Ok(Value::str(&date_time_string(&r, cal, precision, show)))
}

fn to_json(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.toJSON")?;
    Ok(Value::str(&date_time_string(&dt, cal, None, ShowCalendar::Auto)))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, _) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.toLocaleString")?;
    locale_string(vm, &dt, &a.arg(0), &a.arg(1), "any", "all")
}

fn value_of(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    super::value_of_throws(vm, "PlainDateTime")
}

fn to_zoned_date_time(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.toZonedDateTime")?;
    let tz = zone::to_time_zone(vm, &a.arg(0))?;
    let o = options::get_options(vm, &a.arg(1))?;
    let dis = options::disambiguation(vm, &o)?;
    let ns = zone::epoch_for(vm, &tz, &dt, dis)?;
    create_zoned(vm, ns, tz, cal, None)
}

fn to_plain_date(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, cal) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.toPlainDate")?;
    create_date(vm, dt.date, cal, None)
}

fn to_plain_time(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (dt, _) = this_date_time(vm, a, "Temporal.PlainDateTime.prototype.toPlainTime")?;
    create_time(vm, dt.time, None)
}
