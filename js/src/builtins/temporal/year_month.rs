//! Temporal.PlainYearMonth: um mês de um ano (sem dia).

use super::calendar::{self, Cal, DateField, F_DAY, FieldsKind, Required, ShowCalendar, YEAR_MONTH_FIELDS};
use super::duration;
use super::instant::{difference_result, difference_settings};
use super::iso::{self, IsoDate, MIDNIGHT};
use super::options::{self, Group};
use super::parse::{self, Goal};
use super::plain_date::ctor_calendar;
use super::round::{self, InternalDur, Unit};
use super::{
    TemporalData, check_partial, class, create_date, create_year_month, data_getter, getter_index, incompatible, locale_string,
    range, sat, this_year_month, to_int_trunc, type_err,
};
use crate::builtins::Realm;
use crate::value::*;
use crate::vm::Vm;

const GETTERS: [(&str, DateField); 9] = [
    ("era", DateField::Era),
    ("eraYear", DateField::EraYear),
    ("year", DateField::Year),
    ("month", DateField::Month),
    ("monthCode", DateField::MonthCode),
    ("daysInYear", DateField::DaysInYear),
    ("daysInMonth", DateField::DaysInMonth),
    ("monthsInYear", DateField::MonthsInYear),
    ("inLeapYear", DateField::InLeapYear),
];

pub fn install(r: &Realm, ns: &ObjRef, p: &ObjRef) {
    let c = class(r, ns, "PlainYearMonth", 2, ctor, p);
    r.method(&c, "from", 1, from);
    r.method(&c, "compare", 2, compare);
    r.getter(p, "calendarId", calendar_id);
    for (k, (name, _)) in GETTERS.iter().enumerate() {
        data_getter(r, p, name, getter, k);
    }
    r.method(p, "with", 1, with);
    r.method(p, "add", 1, add);
    r.method(p, "subtract", 1, subtract);
    r.method(p, "until", 1, until);
    r.method(p, "since", 1, since);
    r.method(p, "equals", 1, equals);
    r.method(p, "toString", 0, to_string);
    r.method(p, "toLocaleString", 0, to_locale_string);
    r.method(p, "toJSON", 0, to_json);
    r.method(p, "valueOf", 0, value_of);
    r.method(p, "toPlainDate", 1, to_plain_date);
}

/// ToTemporalYearMonth.
pub fn to_year_month(vm: &mut Vm, v: &Value, options: &Value) -> Result<(IsoDate, Cal), Value> {
    if let Value::Object(_) = v {
        if let Some(TemporalData::YearMonth(d, c)) = super::data(v) {
            let o = options::get_options(vm, options)?;
            options::overflow(vm, &o)?;
            return Ok((d, c));
        }
        let cal = calendar::calendar_with_iso_default(vm, v)?;
        let f = calendar::prepare(vm, cal, v, YEAR_MONTH_FIELDS, Required::None)?;
        let o = options::get_options(vm, options)?;
        let constrain = options::overflow(vm, &o)?;
        let d = calendar::year_month_from_fields(vm, cal, f, constrain)?;
        return Ok((d, cal));
    }
    let Value::String(s) = v else {
        let d = vm.describe(v);
        return type_err(vm, format!("Invalid plain year-month: {d}"));
    };
    let s = s.to_rust();
    let p = match parse::parse_iso(&s, &[Goal::YearMonth]) {
        Ok(p) => p,
        Err(e) => return range(vm, e),
    };
    let cal = calendar::canonicalize_or_throw(vm, p.calendar.as_deref().unwrap_or("iso8601"))?;
    let o = options::get_options(vm, options)?;
    options::overflow(vm, &o)?;
    let d = IsoDate::new(p.year.unwrap_or(0), p.month, p.day);
    if !iso::year_month_within_limits(d.year, d.month) {
        return range(vm, format!("Year and month are outside of supported range: {s}"));
    }
    let f = calendar::date_to_fields(cal, &d, FieldsKind::YearMonth);
    let d = calendar::year_month_from_fields(vm, cal, f, true)?;
    Ok((d, cal))
}

fn ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return type_err(vm, "Constructor Temporal.PlainYearMonth requires 'new'");
    };
    let y = to_int_trunc(vm, &a.arg(0), "year")?;
    let m = to_int_trunc(vm, &a.arg(1), "month")?;
    let cal = ctor_calendar(vm, &a.arg(2))?;
    let rd = a.arg(3);
    let d = if rd.is_undefined() { 1.0 } else { to_int_trunc(vm, &rd, "referenceISODay")? };
    let Some(date) = iso::regulate_date(sat(y), sat(m), sat(d), false) else {
        return range(vm, "Invalid ISO date");
    };
    create_year_month(vm, date, cal, Some(&nt))
}

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = to_year_month(vm, &a.arg(0), &a.arg(1))?;
    create_year_month(vm, d, cal, None)
}

fn compare(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (one, _) = to_year_month(vm, &a.arg(0), &Value::Undefined)?;
    let (two, _) = to_year_month(vm, &a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(iso::cmp_date(&one, &two) as f64))
}

fn calendar_id(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, cal) = this_year_month(vm, a, "Temporal.PlainYearMonth.prototype.calendarId")?;
    Ok(Value::str(cal.id()))
}

fn getter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (name, field) = GETTERS[getter_index(a)];
    match super::data(&a.this) {
        Some(TemporalData::YearMonth(d, c)) => Ok(calendar::date_field(&d, c, field)),
        _ => incompatible(vm, a, &format!("Temporal.PlainYearMonth.prototype.{name}")),
    }
}

fn with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_year_month(vm, a, "Temporal.PlainYearMonth.prototype.with")?;
    let like = a.arg(0);
    check_partial(vm, &like)?;
    let fields = calendar::date_to_fields(cal, &d, FieldsKind::YearMonth);
    let partial = calendar::prepare(vm, cal, &like, YEAR_MONTH_FIELDS, Required::Partial)?;
    let merged = calendar::merge(cal, &fields, &partial);
    let o = options::get_options(vm, &a.arg(1))?;
    let constrain = options::overflow(vm, &o)?;
    let r = calendar::year_month_from_fields(vm, cal, merged, constrain)?;
    create_year_month(vm, r, cal, None)
}

/// AddDurationToYearMonth.
fn add_duration(vm: &mut Vm, a: &CallArgs, sub: bool, method: &str) -> Result<Value, Value> {
    let (d, cal) = this_year_month(vm, a, method)?;
    let mut f = duration::to_duration(vm, &a.arg(0))?;
    if sub {
        f = duration::negate(&f);
    }
    let internal = duration::to_internal(&f);
    let o = options::get_options(vm, &a.arg(1))?;
    let constrain = options::overflow(vm, &o)?;
    if internal.date.weeks != 0 || internal.date.days != 0 || internal.time != 0 {
        return range(vm, "Only years and months can be added to Temporal.PlainYearMonth");
    }
    let mut fields = calendar::date_to_fields(cal, &d, FieldsKind::YearMonth);
    fields.day = Some(1.0);
    let start = calendar::date_from_fields(vm, cal, fields, true)?;
    let added = calendar::date_add(vm, cal, &start, &internal.date, constrain)?;
    let af = calendar::date_to_fields(cal, &added, FieldsKind::YearMonth);
    let r = calendar::year_month_from_fields(vm, cal, af, constrain)?;
    create_year_month(vm, r, cal, None)
}

fn add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, false, "Temporal.PlainYearMonth.prototype.add")
}

fn subtract(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, true, "Temporal.PlainYearMonth.prototype.subtract")
}

fn difference(vm: &mut Vm, a: &CallArgs, since: bool, method: &str) -> Result<Value, Value> {
    let (d, cal) = this_year_month(vm, a, method)?;
    let (other, ocal) = to_year_month(vm, &a.arg(0), &Value::Undefined)?;
    if cal != ocal {
        return range(vm, format!("Cannot compute difference between months of {} and {} calendars", cal.id(), ocal.id()));
    }
    let o = options::get_options(vm, &a.arg(1))?;
    let s = difference_settings(vm, since, &o, Group::Date, &[Unit::Week, Unit::Day], Unit::Month, Unit::Year)?;
    if d == other {
        return super::create_duration(vm, [0.0; 10], None);
    }
    let first = |vm: &mut Vm, x: &IsoDate| {
        let mut f = calendar::date_to_fields(cal, x, FieldsKind::YearMonth);
        f.day = Some(1.0);
        calendar::date_from_fields(vm, cal, f, true)
    };
    let this_date = first(vm, &d)?;
    let other_date = first(vm, &other)?;
    let dd = calendar::date_until(vm, cal, &this_date, &other_date, s.largest);
    let mut r = InternalDur {
        date: round::adjust(vm, &dd, 0, Some(0), None)?,
        time: 0,
    };
    if s.smallest != Unit::Month || s.inc != 1 {
        let (a1, b1) = (this_date.at(MIDNIGHT), other_date.at(MIDNIGHT));
        r = round::round_relative(vm, &r, iso::epoch_ns(&a1), iso::epoch_ns(&b1), &a1, None, cal, s.largest, s.inc, s.smallest, s.mode)?;
    }
    difference_result(vm, &r, Unit::Day, since)
}

fn until(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, false, "Temporal.PlainYearMonth.prototype.until")
}

fn since(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, true, "Temporal.PlainYearMonth.prototype.since")
}

fn equals(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_year_month(vm, a, "Temporal.PlainYearMonth.prototype.equals")?;
    let (other, ocal) = to_year_month(vm, &a.arg(0), &Value::Undefined)?;
    Ok(Value::Bool(d == other && cal == ocal))
}

/// TemporalYearMonthToString.
pub fn year_month_string(d: &IsoDate, cal: Cal, show: ShowCalendar) -> String {
    let mut s = format!("{}-{:02}", iso::year_string(d.year), d.month);
    if matches!(show, ShowCalendar::Always | ShowCalendar::Critical) || cal != Cal::Iso {
        s.push_str(&format!("-{:02}", d.day));
    }
    s.push_str(&calendar::annotation(cal, show));
    s
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_year_month(vm, a, "Temporal.PlainYearMonth.prototype.toString")?;
    let o = options::get_options(vm, &a.arg(0))?;
    let show = options::show_calendar(vm, &o)?;
    Ok(Value::str(&year_month_string(&d, cal, show)))
}

fn to_json(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_year_month(vm, a, "Temporal.PlainYearMonth.prototype.toJSON")?;
    Ok(Value::str(&year_month_string(&d, cal, ShowCalendar::Auto)))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    this_year_month(vm, a, "Temporal.PlainYearMonth.prototype.toLocaleString")?;
    locale_string(vm, a, "date", "date")
}

fn value_of(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    super::value_of_throws(vm, "PlainYearMonth")
}

fn to_plain_date(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (d, cal) = this_year_month(vm, a, "Temporal.PlainYearMonth.prototype.toPlainDate")?;
    let item = a.arg(0);
    if !matches!(item, Value::Object(_)) {
        let s = vm.describe(&item);
        return type_err(vm, format!("Invalid argument: {s} is not an object"));
    }
    let fields = calendar::date_to_fields(cal, &d, FieldsKind::YearMonth);
    let input = calendar::prepare(vm, cal, &item, F_DAY, Required::None)?;
    let merged = calendar::merge(cal, &fields, &input);
    let r = calendar::date_from_fields(vm, cal, merged, true)?;
    create_date(vm, r, cal, None)
}
