//! Temporal.PlainMonthDay: dia de um mês, sem ano (aniversários, feriados).

use super::calendar::{self, Cal, DATE_FIELDS, DateField, F_YEAR, FieldsKind, Required, ShowCalendar};
use super::iso::{self, IsoDate};
use super::options;
use super::parse::{self, Goal};
use super::plain_date::ctor_calendar;
use super::{
    TemporalData, check_partial, class, create_date, create_month_day, data_getter, getter_index, incompatible, locale_string, range,
    sat, this_month_day, to_int_trunc, type_err,
};
use crate::builtins::Realm;
use crate::value::*;
use crate::vm::Vm;

const GETTERS: [(&str, DateField); 2] = [("monthCode", DateField::MonthCode), ("day", DateField::Day)];

pub fn install(r: &Realm, ns: &ObjRef, p: &ObjRef) {
    let c = class(r, ns, "PlainMonthDay", 2, ctor, p);
    r.method(&c, "from", 1, from);
    r.getter(p, "calendarId", calendar_id);
    for (k, (name, _)) in GETTERS.iter().enumerate() {
        data_getter(r, p, name, getter, k);
    }
    r.method(p, "with", 1, with);
    r.method(p, "equals", 1, equals);
    r.method(p, "toString", 0, to_string);
    r.method(p, "toLocaleString", 0, to_locale_string);
    r.method(p, "toJSON", 0, to_json);
    r.method(p, "valueOf", 0, value_of);
    r.method(p, "toPlainDate", 1, to_plain_date);
}

/// ToTemporalMonthDay.
pub fn to_month_day(vm: &mut Vm, v: &Value, options: &Value) -> Result<(IsoDate, Cal), Value> {
    if let Value::Object(_) = v {
        if let Some(TemporalData::MonthDay(d, c)) = super::data(v) {
            let o = options::get_options(vm, options)?;
            options::overflow(vm, &o)?;
            return Ok((d, c));
        }
        let cal = calendar::calendar_with_iso_default(vm, v)?;
        let f = calendar::prepare(vm, cal, v, DATE_FIELDS, Required::None)?;
        let o = options::get_options(vm, options)?;
        let constrain = options::overflow(vm, &o)?;
        let d = calendar::month_day_from_fields(vm, cal, f, constrain)?;
        return Ok((d, cal));
    }
    let Value::String(s) = v else {
        let d = vm.describe(v);
        return type_err(vm, format!("Invalid plain month-day: {d}"));
    };
    let s = s.to_rust();
    let p = match parse::parse_iso(&s, &[Goal::MonthDay]) {
        Ok(p) => p,
        Err(e) => return range(vm, e),
    };
    let cal = calendar::canonicalize_or_throw(vm, p.calendar.as_deref().unwrap_or("iso8601"))?;
    let o = options::get_options(vm, options)?;
    options::overflow(vm, &o)?;
    if cal == Cal::Iso {
        return Ok((IsoDate::new(1972, p.month, p.day), cal));
    }
    // outro calendário: o mês e dia daquele calendário na data ISO dada
    let d = IsoDate::new(p.year.unwrap_or(1972), p.month, p.day);
    if !iso::date_within_limits(&d) {
        return range(vm, format!("Date is outside of supported range: {s}"));
    }
    let f = calendar::date_to_fields(cal, &d, FieldsKind::MonthDay);
    Ok((calendar::month_day_from_fields(vm, cal, f, true)?, cal))
}

fn ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return type_err(vm, "Constructor Temporal.PlainMonthDay requires 'new'");
    };
    let m = to_int_trunc(vm, &a.arg(0), "month")?;
    let d = to_int_trunc(vm, &a.arg(1), "day")?;
    let cal = ctor_calendar(vm, &a.arg(2))?;
    let ry = a.arg(3);
    let y = if ry.is_undefined() { 1972.0 } else { to_int_trunc(vm, &ry, "referenceISOYear")? };
    let Some(date) = iso::regulate_date(sat(y), sat(m), sat(d), false) else {
        return range(vm, "Invalid ISO date");
    };
    create_month_day(vm, date, cal, Some(&nt))
}

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = to_month_day(vm, &a.arg(0), &a.arg(1))?;
    create_month_day(vm, d, cal, None)
}

fn calendar_id(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, cal) = this_month_day(vm, a, "Temporal.PlainMonthDay.prototype.calendarId")?;
    Ok(Value::str(cal.id()))
}

fn getter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (name, field) = GETTERS[getter_index(a)];
    match super::data(&a.this) {
        Some(TemporalData::MonthDay(d, c)) => Ok(calendar::date_field(&d, c, field)),
        _ => incompatible(vm, a, &format!("Temporal.PlainMonthDay.prototype.{name}")),
    }
}

fn with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_month_day(vm, a, "Temporal.PlainMonthDay.prototype.with")?;
    let like = a.arg(0);
    check_partial(vm, &like)?;
    let fields = calendar::date_to_fields(cal, &d, FieldsKind::MonthDay);
    let partial = calendar::prepare(vm, cal, &like, DATE_FIELDS, Required::Partial)?;
    let merged = calendar::merge(cal, &fields, &partial);
    let o = options::get_options(vm, &a.arg(1))?;
    let constrain = options::overflow(vm, &o)?;
    let r = calendar::month_day_from_fields(vm, cal, merged, constrain)?;
    create_month_day(vm, r, cal, None)
}

fn equals(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_month_day(vm, a, "Temporal.PlainMonthDay.prototype.equals")?;
    let (other, ocal) = to_month_day(vm, &a.arg(0), &Value::Undefined)?;
    Ok(Value::Bool(d == other && cal == ocal))
}

/// TemporalMonthDayToString.
pub fn month_day_string(d: &IsoDate, cal: Cal, show: ShowCalendar) -> String {
    let mut s = format!("{:02}-{:02}", d.month, d.day);
    if matches!(show, ShowCalendar::Always | ShowCalendar::Critical) || cal != Cal::Iso {
        s = format!("{}-{s}", iso::year_string(d.year));
    }
    s.push_str(&calendar::annotation(cal, show));
    s
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_month_day(vm, a, "Temporal.PlainMonthDay.prototype.toString")?;
    let o = options::get_options(vm, &a.arg(0))?;
    let show = options::show_calendar(vm, &o)?;
    Ok(Value::str(&month_day_string(&d, cal, show)))
}

fn to_json(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_month_day(vm, a, "Temporal.PlainMonthDay.prototype.toJSON")?;
    Ok(Value::str(&month_day_string(&d, cal, ShowCalendar::Auto)))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, _) = this_month_day(vm, a, "Temporal.PlainMonthDay.prototype.toLocaleString")?;
    locale_string(vm, &d.at(iso::NOON), &a.arg(0), &a.arg(1), "date", "date")
}

fn value_of(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    super::value_of_throws(vm, "PlainMonthDay")
}

fn to_plain_date(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_month_day(vm, a, "Temporal.PlainMonthDay.prototype.toPlainDate")?;
    let item = a.arg(0);
    if !matches!(item, Value::Object(_)) {
        let s = vm.describe(&item);
        return type_err(vm, format!("Invalid argument: {s} is not an object"));
    }
    let fields = calendar::date_to_fields(cal, &d, FieldsKind::MonthDay);
    let input = calendar::prepare(vm, cal, &item, F_YEAR, Required::None)?;
    let merged = calendar::merge(cal, &fields, &input);
    let r = calendar::date_from_fields(vm, cal, merged, true)?;
    create_date(vm, r, cal, None)
}
