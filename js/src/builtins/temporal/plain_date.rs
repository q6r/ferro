//! Temporal.PlainDate: data de calendário, sem hora nem fuso.

use super::calendar::{self, Cal, DATE_FIELDS, DATE_GETTERS, DateDur, FieldsKind, Required};
use super::duration;
use super::instant::{difference_result, difference_settings};
use super::iso::{self, IsoDate, MIDNIGHT, NS_PER_DAY};
use super::options::{self, Group};
use super::parse::{self, Goal};
use super::plain_time::{to_time, to_time_or_midnight};
use super::round::{self, InternalDur, Unit};
use super::zone::{self, Disambig};
use super::{
    TemporalData, check_partial, class, create_date, create_date_time, create_month_day, create_year_month, create_zoned, data_getter,
    incompatible, locale_string, range, this_date, to_int_trunc, type_err,
};
use crate::builtins::Realm;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm, ns: &ObjRef, p: &ObjRef) {
    let c = class(r, ns, "PlainDate", 3, ctor, p);
    r.method(&c, "from", 1, from);
    r.method(&c, "compare", 2, compare);
    r.getter(p, "calendarId", calendar_id);
    for (k, (name, _)) in DATE_GETTERS.iter().enumerate() {
        data_getter(r, p, name, date_getter, k);
    }
    r.method(p, "toPlainYearMonth", 0, to_plain_year_month);
    r.method(p, "toPlainMonthDay", 0, to_plain_month_day);
    r.method(p, "add", 1, add);
    r.method(p, "subtract", 1, subtract);
    r.method(p, "with", 1, with);
    r.method(p, "withCalendar", 1, with_calendar);
    r.method(p, "until", 1, until);
    r.method(p, "since", 1, since);
    r.method(p, "equals", 1, equals);
    r.method(p, "toPlainDateTime", 0, to_plain_date_time);
    r.method(p, "toZonedDateTime", 1, to_zoned_date_time);
    r.method(p, "toString", 0, to_string);
    r.method(p, "toLocaleString", 0, to_locale_string);
    r.method(p, "toJSON", 0, to_json);
    r.method(p, "valueOf", 0, value_of);
}

/// Construtor comum das classes com calendário: string ou undefined.
pub fn ctor_calendar(vm: &mut Vm, v: &Value) -> Result<Cal, Value> {
    match v {
        Value::Undefined => Ok(Cal::Iso),
        Value::String(s) => {
            let s = s.to_rust();
            calendar::canonicalize_or_throw(vm, &s)
        }
        _ => {
            let d = vm.describe(v);
            type_err(vm, format!("Invalid calendar: {d} is not a string"))
        }
    }
}

/// ToTemporalDate.
pub fn to_date(vm: &mut Vm, v: &Value, options: &Value) -> Result<(IsoDate, Cal), Value> {
    if let Value::Object(_) = v {
        let direct = match super::data(v) {
            Some(TemporalData::Date(d, c)) => Some((d, c)),
            Some(TemporalData::Zoned(ns, tz, c)) => Some((zone::date_time_for(vm, &tz, ns).date, c)),
            Some(TemporalData::DateTime(dt, c)) => Some((dt.date, c)),
            _ => None,
        };
        if let Some(r) = direct {
            let o = options::get_options(vm, options)?;
            options::overflow(vm, &o)?;
            return Ok(r);
        }
        let cal = calendar::calendar_with_iso_default(vm, v)?;
        let f = calendar::prepare(vm, cal, v, DATE_FIELDS, Required::None)?;
        let o = options::get_options(vm, options)?;
        let constrain = options::overflow(vm, &o)?;
        let d = calendar::date_from_fields(vm, cal, f, constrain)?;
        return Ok((d, cal));
    }
    let Value::String(s) = v else {
        let d = vm.describe(v);
        return type_err(vm, format!("Invalid plain date: {d}"));
    };
    let s = s.to_rust();
    let p = match parse::parse_iso(&s, &[Goal::DateTime]) {
        Ok(p) => p,
        Err(e) => return range(vm, e),
    };
    let cal = calendar::canonicalize_or_throw(vm, p.calendar.as_deref().unwrap_or("iso8601"))?;
    let o = options::get_options(vm, options)?;
    options::overflow(vm, &o)?;
    let d = IsoDate::new(p.year.unwrap_or(0), p.month, p.day);
    if !iso::date_within_limits(&d) {
        return range(vm, format!("Date is outside of supported range: {s}"));
    }
    Ok((d, cal))
}

fn ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return type_err(vm, "Constructor Temporal.PlainDate requires 'new'");
    };
    let y = to_int_trunc(vm, &a.arg(0), "year")?;
    let m = to_int_trunc(vm, &a.arg(1), "month")?;
    let d = to_int_trunc(vm, &a.arg(2), "day")?;
    let cal = ctor_calendar(vm, &a.arg(3))?;
    let Some(date) = iso::regulate_date(super::sat(y), super::sat(m), super::sat(d), false) else {
        return range(vm, "Invalid ISO date");
    };
    create_date(vm, date, cal, Some(&nt))
}

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = to_date(vm, &a.arg(0), &a.arg(1))?;
    create_date(vm, d, cal, None)
}

fn compare(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (one, _) = to_date(vm, &a.arg(0), &Value::Undefined)?;
    let (two, _) = to_date(vm, &a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(iso::cmp_date(&one, &two) as f64))
}

fn calendar_id(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, cal) = this_date(vm, a, "Temporal.PlainDate.prototype.calendarId")?;
    Ok(Value::str(cal.id()))
}

fn date_getter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (name, field) = DATE_GETTERS[super::getter_index(a)];
    match super::data(&a.this) {
        Some(TemporalData::Date(d, c)) => Ok(calendar::date_field(&d, c, field)),
        _ => incompatible(vm, a, &format!("Temporal.PlainDate.prototype.{name}")),
    }
}

fn to_plain_year_month(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, "Temporal.PlainDate.prototype.toPlainYearMonth")?;
    let f = calendar::date_to_fields(cal, &d, FieldsKind::Date);
    let r = calendar::year_month_from_fields(vm, cal, f, true)?;
    create_year_month(vm, r, cal, None)
}

fn to_plain_month_day(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, "Temporal.PlainDate.prototype.toPlainMonthDay")?;
    let f = calendar::date_to_fields(cal, &d, FieldsKind::Date);
    let r = calendar::month_day_from_fields(vm, cal, f, true)?;
    create_month_day(vm, r, cal, None)
}

/// ToDateDurationRecordWithoutTime.
pub fn date_duration_without_time(vm: &mut Vm, f: &[f64; 10]) -> Result<DateDur, Value> {
    let d = duration::to_internal_24h(f);
    round::adjust(vm, &d.date, (d.time / NS_PER_DAY) as i64, None, None)
}

/// AddDurationToDate.
fn add_duration(vm: &mut Vm, a: &CallArgs, sub: bool, method: &str) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, method)?;
    let mut f = duration::to_duration(vm, &a.arg(0))?;
    if sub {
        f = duration::negate(&f);
    }
    let dd = date_duration_without_time(vm, &f)?;
    let o = options::get_options(vm, &a.arg(1))?;
    let constrain = options::overflow(vm, &o)?;
    let r = calendar::date_add(vm, cal, &d, &dd, constrain)?;
    create_date(vm, r, cal, None)
}

fn add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, false, "Temporal.PlainDate.prototype.add")
}

fn subtract(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, true, "Temporal.PlainDate.prototype.subtract")
}

fn with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, "Temporal.PlainDate.prototype.with")?;
    let like = a.arg(0);
    check_partial(vm, &like)?;
    let fields = calendar::date_to_fields(cal, &d, FieldsKind::Date);
    let partial = calendar::prepare(vm, cal, &like, DATE_FIELDS, Required::Partial)?;
    let merged = calendar::merge(cal, &fields, &partial);
    let o = options::get_options(vm, &a.arg(1))?;
    let constrain = options::overflow(vm, &o)?;
    let r = calendar::date_from_fields(vm, cal, merged, constrain)?;
    create_date(vm, r, cal, None)
}

fn with_calendar(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, _) = this_date(vm, a, "Temporal.PlainDate.prototype.withCalendar")?;
    let cal = calendar::to_calendar(vm, &a.arg(0))?;
    create_date(vm, d, cal, None)
}

fn difference(vm: &mut Vm, a: &CallArgs, since: bool, method: &str) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, method)?;
    let (other, ocal) = to_date(vm, &a.arg(0), &Value::Undefined)?;
    if cal != ocal {
        return range(vm, format!("Cannot compute difference between dates of {} and {} calendars", cal.id(), ocal.id()));
    }
    let o = options::get_options(vm, &a.arg(1))?;
    let s = difference_settings(vm, since, &o, Group::Date, &[], Unit::Day, Unit::Day)?;
    if d == other {
        return super::create_duration(vm, [0.0; 10], None);
    }
    let mut r = InternalDur {
        date: calendar::date_until(vm, cal, &d, &other, s.largest),
        time: 0,
    };
    if !(s.smallest == Unit::Day && s.inc == 1) {
        let (a1, b1) = (d.at(MIDNIGHT), other.at(MIDNIGHT));
        r = round::round_relative(vm, &r, iso::epoch_ns(&a1), iso::epoch_ns(&b1), &a1, None, cal, s.largest, s.inc, s.smallest, s.mode)?;
    }
    difference_result(vm, &r, Unit::Day, since)
}

fn until(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, false, "Temporal.PlainDate.prototype.until")
}

fn since(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, true, "Temporal.PlainDate.prototype.since")
}

fn equals(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, "Temporal.PlainDate.prototype.equals")?;
    let (other, ocal) = to_date(vm, &a.arg(0), &Value::Undefined)?;
    Ok(Value::Bool(d == other && cal == ocal))
}

fn to_plain_date_time(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, "Temporal.PlainDate.prototype.toPlainDateTime")?;
    let t = to_time_or_midnight(vm, &a.arg(0))?;
    create_date_time(vm, d.at(t), cal, None)
}

fn to_zoned_date_time(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, "Temporal.PlainDate.prototype.toZonedDateTime")?;
    let item = a.arg(0);
    let (tz, time) = if let Value::Object(_) = &item {
        let like = vm.get_str(&item, "timeZone")?;
        if like.is_undefined() {
            (zone::to_time_zone(vm, &item)?, Value::Undefined)
        } else {
            let tz = zone::to_time_zone(vm, &like)?;
            (tz, vm.get_str(&item, "plainTime")?)
        }
    } else {
        (zone::to_time_zone(vm, &item)?, Value::Undefined)
    };
    let ns = if time.is_undefined() {
        zone::start_of_day(vm, &tz, &d)?
    } else {
        let t = to_time(vm, &time, &Value::Undefined)?;
        let dt = d.at(t);
        if !iso::date_time_within_limits(&dt) {
            return range(vm, "Date and time are outside of supported range");
        }
        zone::epoch_for(vm, &tz, &dt, Disambig::Compatible)?
    };
    create_zoned(vm, ns, tz, cal, None)
}

pub fn date_string(d: &IsoDate, cal: Cal, show: calendar::ShowCalendar) -> String {
    format!("{}{}", iso::date_string(d), calendar::annotation(cal, show))
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, "Temporal.PlainDate.prototype.toString")?;
    let o = options::get_options(vm, &a.arg(0))?;
    let show = options::show_calendar(vm, &o)?;
    Ok(Value::str(&date_string(&d, cal, show)))
}

fn to_json(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_date(vm, a, "Temporal.PlainDate.prototype.toJSON")?;
    Ok(Value::str(&date_string(&d, cal, calendar::ShowCalendar::Auto)))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, _) = this_date(vm, a, "Temporal.PlainDate.prototype.toLocaleString")?;
    locale_string(vm, &d.at(iso::NOON), &a.arg(0), &a.arg(1), "date", "date")
}

fn value_of(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    super::value_of_throws(vm, "PlainDate")
}
