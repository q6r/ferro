//! Temporal.ZonedDateTime: instante exato com fuso e calendário.

use super::calendar::{self, Cal, DATE_FIELDS, DATE_GETTERS, F_OFFSET, F_TIME_ZONE, Required, ShowCalendar, TIME_FIELDS};
use super::duration;
use super::instant::{difference_result, difference_settings};
use super::iso::{self, IsoDate};
use super::options::{self, Group, UnitOpt};
use super::parse::{self, Goal, TzId};
use super::plain_date_time::{date_time_fields, round_settings, time_field};
use super::plain_time::to_time;
use super::round::{self, RoundingMode, Unit};
use super::zone::{self, Disambig, OffsetBehaviour, OffsetOption, Tz};
use super::{
    TemporalData, bigint_from_i128, check_partial, class, create_date, create_date_time, create_instant, create_time, create_zoned,
    data_getter, getter_index, incompatible, range, this_zoned, type_err,
};
use crate::builtins::Realm;
use crate::value::*;
use crate::vm::Vm;

const TIME_GETTERS: [&str; 6] = ["hour", "minute", "second", "millisecond", "microsecond", "nanosecond"];

pub fn install(r: &Realm, ns: &ObjRef, p: &ObjRef) {
    let c = class(r, ns, "ZonedDateTime", 2, ctor, p);
    r.method(&c, "from", 1, from);
    r.method(&c, "compare", 2, compare);
    r.getter(p, "calendarId", calendar_id);
    r.getter(p, "timeZoneId", time_zone_id);
    for (k, (name, _)) in DATE_GETTERS.iter().enumerate() {
        data_getter(r, p, name, date_getter, k);
    }
    for (k, name) in TIME_GETTERS.iter().enumerate() {
        data_getter(r, p, name, time_getter, k);
    }
    r.getter(p, "epochMilliseconds", epoch_ms);
    r.getter(p, "epochNanoseconds", epoch_ns);
    r.getter(p, "hoursInDay", hours_in_day);
    r.getter(p, "offsetNanoseconds", offset_nanoseconds);
    r.getter(p, "offset", offset);
    r.method(p, "with", 1, with);
    r.method(p, "withPlainTime", 0, with_plain_time);
    r.method(p, "withTimeZone", 1, with_time_zone);
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
    r.method(p, "startOfDay", 0, start_of_day);
    r.method(p, "getTimeZoneTransition", 1, get_time_zone_transition);
    r.method(p, "toInstant", 0, to_instant);
    r.method(p, "toPlainDate", 0, to_plain_date);
    r.method(p, "toPlainTime", 0, to_plain_time);
    r.method(p, "toPlainDateTime", 0, to_plain_date_time);
}

/// ToTemporalZonedDateTime.
pub fn to_zoned(vm: &mut Vm, v: &Value, options: &Value) -> Result<(i128, Tz, Cal), Value> {
    let (date, time, tz, offset, cal, has_utc, match_minutes, dis, off_opt);
    if let Value::Object(_) = v {
        if let Some(TemporalData::Zoned(ns, tz, c)) = super::data(v) {
            let o = options::get_options(vm, options)?;
            options::disambiguation(vm, &o)?;
            options::offset_option(vm, &o, OffsetOption::Reject)?;
            options::overflow(vm, &o)?;
            return Ok((ns, tz, c));
        }
        cal = calendar::calendar_with_iso_default(vm, v)?;
        let names = DATE_FIELDS | TIME_FIELDS | F_OFFSET | F_TIME_ZONE;
        let f = calendar::prepare(vm, cal, v, names, Required::TimeZone)?;
        let o = options::get_options(vm, options)?;
        dis = options::disambiguation(vm, &o)?;
        off_opt = options::offset_option(vm, &o, OffsetOption::Reject)?;
        let constrain = options::overflow(vm, &o)?;
        let dt = calendar::interpret_date_time(vm, cal, &f, constrain)?;
        date = dt.date;
        time = Some(dt.time);
        tz = f.time_zone.clone().unwrap_or_else(Tz::utc);
        offset = f.offset.clone();
        has_utc = false;
        match_minutes = false;
    } else {
        let Value::String(s) = v else {
            let d = vm.describe(v);
            return type_err(vm, format!("Invalid zoned date-time: {d}"));
        };
        let s = s.to_rust();
        let p = match parse::parse_iso(&s, &[Goal::Zoned]) {
            Ok(p) => p,
            Err(e) => return range(vm, e),
        };
        tz = zone::from_string(vm, p.tz.as_deref().unwrap_or("UTC"))?;
        offset = p.offset.clone();
        has_utc = p.z;
        cal = calendar::canonicalize_or_throw(vm, p.calendar.as_deref().unwrap_or("iso8601"))?;
        match_minutes = !offset.as_deref().is_some_and(parse::offset_has_seconds);
        let o = options::get_options(vm, options)?;
        dis = options::disambiguation(vm, &o)?;
        off_opt = options::offset_option(vm, &o, OffsetOption::Reject)?;
        options::overflow(vm, &o)?;
        date = IsoDate::new(p.year.unwrap_or(0), p.month, p.day);
        time = p.time;
    }
    let behaviour = if has_utc {
        OffsetBehaviour::Exact
    } else if offset.is_none() {
        OffsetBehaviour::Wall
    } else {
        OffsetBehaviour::Option
    };
    let off = match (behaviour, &offset) {
        (OffsetBehaviour::Option, Some(o)) => parse::parse_utc_offset(o).unwrap_or(0),
        _ => 0,
    };
    let ns = zone::interpret_offset(vm, date, time, behaviour, off, &tz, dis, off_opt, match_minutes)?;
    Ok((ns, tz, cal))
}

fn ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return type_err(vm, "Constructor Temporal.ZonedDateTime requires 'new'");
    };
    let b = super::super::bigint::to_bigint(vm, &a.arg(0))?;
    let ns = match super::bigint_to_i128(&b) {
        Some(n) if iso::is_valid_epoch_ns(n) => n,
        _ => return range(vm, "Invalid epoch nanoseconds: outside of supported range"),
    };
    let tz_v = a.arg(1);
    let Value::String(tz_s) = &tz_v else {
        let d = vm.describe(&tz_v);
        return type_err(vm, format!("Invalid time zone: {d} is not a string"));
    };
    let tz_s = tz_s.to_rust();
    let tz = match parse::parse_tz_identifier(&tz_s) {
        Some(TzId::Offset(m)) => Tz::Offset(m),
        Some(TzId::Name(n)) => zone::resolve_name(vm, &n)?,
        None => return range(vm, format!("Invalid time zone: {tz_s}")),
    };
    let cal = super::plain_date::ctor_calendar(vm, &a.arg(2))?;
    create_zoned(vm, ns, tz, cal, Some(&nt))
}

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = to_zoned(vm, &a.arg(0), &a.arg(1))?;
    create_zoned(vm, ns, tz, cal, None)
}

fn compare(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (one, _, _) = to_zoned(vm, &a.arg(0), &Value::Undefined)?;
    let (two, _, _) = to_zoned(vm, &a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(iso::ord(one.cmp(&two)) as f64))
}

fn calendar_id(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, _, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.calendarId")?;
    Ok(Value::str(cal.id()))
}

fn time_zone_id(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (_, tz, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.timeZoneId")?;
    Ok(Value::str(&tz.id()))
}

fn date_getter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (name, field) = DATE_GETTERS[getter_index(a)];
    match super::data(&a.this) {
        Some(TemporalData::Zoned(ns, tz, c)) => {
            let dt = zone::date_time_for(vm, &tz, ns);
            Ok(calendar::date_field(&dt.date, c, field))
        }
        _ => incompatible(vm, a, &format!("Temporal.ZonedDateTime.prototype.{name}")),
    }
}

fn time_getter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = getter_index(a);
    match super::data(&a.this) {
        Some(TemporalData::Zoned(ns, tz, _)) => {
            let dt = zone::date_time_for(vm, &tz, ns);
            Ok(time_field(&dt.time, k))
        }
        _ => incompatible(vm, a, &format!("Temporal.ZonedDateTime.prototype.{}", TIME_GETTERS[k])),
    }
}

fn epoch_ms(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, _, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.epochMilliseconds")?;
    Ok(Value::Number(ns.div_euclid(1_000_000) as f64))
}

fn epoch_ns(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, _, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.epochNanoseconds")?;
    Ok(bigint_from_i128(ns))
}

fn hours_in_day(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.hoursInDay")?;
    let today = zone::date_time_for(vm, &tz, ns).date;
    let tomorrow = iso::add_days(today, 1);
    let start = zone::start_of_day(vm, &tz, &today)?;
    let end = zone::start_of_day(vm, &tz, &tomorrow)?;
    Ok(Value::Number(round::fdiv(end - start, 3_600_000_000_000)))
}

fn offset_nanoseconds(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.offsetNanoseconds")?;
    Ok(Value::Number(zone::offset_ns(vm, &tz, ns) as f64))
}

fn offset(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.offset")?;
    Ok(Value::str(&zone::format_offset_ns(zone::offset_ns(vm, &tz, ns))))
}

fn with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.with")?;
    let like = a.arg(0);
    check_partial(vm, &like)?;
    let off = zone::offset_ns(vm, &tz, ns);
    let dt = zone::date_time_for(vm, &tz, ns);
    let mut fields = date_time_fields(cal, &dt);
    fields.offset = Some(zone::format_offset_ns(off));
    let partial = calendar::prepare(vm, cal, &like, DATE_FIELDS | TIME_FIELDS | F_OFFSET, Required::Partial)?;
    let merged = calendar::merge(cal, &fields, &partial);
    let o = options::get_options(vm, &a.arg(1))?;
    let dis = options::disambiguation(vm, &o)?;
    let off_opt = options::offset_option(vm, &o, OffsetOption::Prefer)?;
    let constrain = options::overflow(vm, &o)?;
    let ndt = calendar::interpret_date_time(vm, cal, &merged, constrain)?;
    let new_off = merged.offset.as_deref().and_then(parse::parse_utc_offset).unwrap_or(0);
    let r = zone::interpret_offset(vm, ndt.date, Some(ndt.time), OffsetBehaviour::Option, new_off, &tz, dis, off_opt, false)?;
    create_zoned(vm, r, tz, cal, None)
}

fn with_plain_time(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.withPlainTime")?;
    let date = zone::date_time_for(vm, &tz, ns).date;
    let t = a.arg(0);
    let r = if t.is_undefined() {
        zone::start_of_day(vm, &tz, &date)?
    } else {
        let time = to_time(vm, &t, &Value::Undefined)?;
        zone::epoch_for(vm, &tz, &date.at(time), Disambig::Compatible)?
    };
    create_zoned(vm, r, tz, cal, None)
}

fn with_time_zone(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, _, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.withTimeZone")?;
    let tz = zone::to_time_zone(vm, &a.arg(0))?;
    create_zoned(vm, ns, tz, cal, None)
}

fn with_calendar(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.withCalendar")?;
    let cal = calendar::to_calendar(vm, &a.arg(0))?;
    create_zoned(vm, ns, tz, cal, None)
}

/// AddDurationToZonedDateTime.
fn add_duration(vm: &mut Vm, a: &CallArgs, sub: bool, method: &str) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, method)?;
    let mut f = duration::to_duration(vm, &a.arg(0))?;
    if sub {
        f = duration::negate(&f);
    }
    let o = options::get_options(vm, &a.arg(1))?;
    let constrain = options::overflow(vm, &o)?;
    let r = round::add_zoned(vm, ns, &tz, cal, &duration::to_internal(&f), constrain)?;
    create_zoned(vm, r, tz, cal, None)
}

fn add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, false, "Temporal.ZonedDateTime.prototype.add")
}

fn subtract(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_duration(vm, a, true, "Temporal.ZonedDateTime.prototype.subtract")
}

fn difference(vm: &mut Vm, a: &CallArgs, since: bool, method: &str) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, method)?;
    let (other, otz, ocal) = to_zoned(vm, &a.arg(0), &Value::Undefined)?;
    if cal != ocal {
        return range(vm, format!("Cannot compute difference between dates of {} and {} calendars", cal.id(), ocal.id()));
    }
    let o = options::get_options(vm, &a.arg(1))?;
    let s = difference_settings(vm, since, &o, Group::DateTime, &[], Unit::Nanosecond, Unit::Hour)?;
    if !s.largest.is_date() {
        let d = round::diff_epoch(vm, ns, other, s.inc, s.smallest, s.mode)?;
        return difference_result(vm, &d, s.largest, since);
    }
    if !tz.equals(&otz) {
        return range(vm, "When calculating difference between time zones, largestUnit must be 'hours' or smaller because day lengths can vary between time zones due to DST or time zone offset changes.");
    }
    if ns == other {
        return super::create_duration(vm, [0.0; 10], None);
    }
    let d = round::diff_zoned_rounded(vm, ns, other, &tz, cal, s.largest, s.inc, s.smallest, s.mode)?;
    difference_result(vm, &d, Unit::Hour, since)
}

fn until(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, false, "Temporal.ZonedDateTime.prototype.until")
}

fn since(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    difference(vm, a, true, "Temporal.ZonedDateTime.prototype.since")
}

fn round(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.round")?;
    let (inc, mode, unit) = round_settings(vm, &a.arg(0))?;
    if unit == Unit::Nanosecond && inc == 1 {
        return create_zoned(vm, ns, tz, cal, None);
    }
    let dt = zone::date_time_for(vm, &tz, ns);
    let r = if unit == Unit::Day {
        let start = zone::start_of_day(vm, &tz, &dt.date)?;
        let end = zone::start_of_day(vm, &tz, &iso::add_days(dt.date, 1))?;
        let this = if ns >= end { end - 1 } else { ns };
        let progress = this - start;
        start + round::round_i128(progress, end - start, mode)
    } else {
        let rd = round::round_date_time(&dt, inc, unit, mode);
        let off = zone::offset_ns(vm, &tz, ns) as i128;
        zone::interpret_offset(vm, rd.date, Some(rd.time), OffsetBehaviour::Option, off, &tz, Disambig::Compatible, OffsetOption::Prefer, false)?
    };
    create_zoned(vm, r, tz, cal, None)
}

fn equals(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.equals")?;
    let (other, otz, ocal) = to_zoned(vm, &a.arg(0), &Value::Undefined)?;
    Ok(Value::Bool(ns == other && tz.equals(&otz) && cal == ocal))
}

/// TemporalZonedDateTimeToString.
#[allow(clippy::too_many_arguments)]
fn zoned_string(
    vm: &mut Vm,
    ns: i128,
    tz: &Tz,
    cal: Cal,
    precision: Option<u8>,
    show_cal: ShowCalendar,
    show_tz: (bool, bool),
    show_offset: bool,
    rounding: Option<(Unit, i64, RoundingMode)>,
) -> String {
    let ns = match rounding {
        Some((unit, inc, mode)) => round::round_positive(ns, unit.ns() * inc as i128, mode),
        None => ns,
    };
    let off = zone::offset_ns(vm, tz, ns);
    let dt = iso::from_epoch_ns(ns + off as i128);
    let mut s = iso::date_time_string(&dt, precision);
    if show_offset {
        s.push_str(&zone::format_offset_rounded(off));
    }
    if show_tz.0 {
        s.push('[');
        if show_tz.1 {
            s.push('!');
        }
        s.push_str(&tz.id());
        s.push(']');
    }
    s.push_str(&calendar::annotation(cal, show_cal));
    s
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.toString")?;
    let o = options::get_options(vm, &a.arg(0))?;
    let show_cal = options::show_calendar(vm, &o)?;
    let digits = options::fractional_digits(vm, &o)?;
    let show_offset = options::show_offset(vm, &o)?;
    let mode = options::rounding_mode(vm, &o, RoundingMode::Trunc)?;
    let smallest = options::unit(vm, &o, "smallestUnit", false)?;
    let show_tz = options::show_time_zone(vm, &o)?;
    options::validate_unit(vm, smallest, Group::Time, &[])?;
    if smallest == UnitOpt::U(Unit::Hour) {
        return range(vm, "smallestUnit must be a time unit other than \"hour\"");
    }
    let (precision, unit, inc) = options::seconds_precision(smallest, digits);
    let s = zoned_string(vm, ns, &tz, cal, precision, show_cal, show_tz, show_offset, Some((unit, inc, mode)));
    Ok(Value::str(&s))
}

fn to_json(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.toJSON")?;
    let s = zoned_string(vm, ns, &tz, cal, None, ShowCalendar::Auto, (true, false), true, None);
    Ok(Value::str(&s))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.toLocaleString")?;
    let dt = zone::date_time_for(vm, &tz, ns);
    super::locale_string(vm, &dt, &a.arg(0), &a.arg(1), "any", "all")
}

fn value_of(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    super::value_of_throws(vm, "ZonedDateTime")
}

fn start_of_day(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.startOfDay")?;
    let date = zone::date_time_for(vm, &tz, ns).date;
    let r = zone::start_of_day(vm, &tz, &date)?;
    create_zoned(vm, r, tz, cal, None)
}

fn get_time_zone_transition(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.getTimeZoneTransition")?;
    let v = a.arg(0);
    if v.is_undefined() {
        return type_err(vm, "options parameter is required");
    }
    let o = if let Value::String(_) = v { Some(super::options_with("direction", v.clone())) } else { options::get_options(vm, &v)? };
    let next = options::direction(vm, &o)?;
    match zone::transition(vm, &tz, ns, next) {
        Some(t) => create_zoned(vm, t, tz, cal, None),
        None => Ok(Value::Null),
    }
}

fn to_instant(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, _, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.toInstant")?;
    create_instant(vm, ns, None)
}

fn to_plain_date(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.toPlainDate")?;
    let dt = zone::date_time_for(vm, &tz, ns);
    create_date(vm, dt.date, cal, None)
}

fn to_plain_time(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, _) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.toPlainTime")?;
    let dt = zone::date_time_for(vm, &tz, ns);
    create_time(vm, dt.time, None)
}

fn to_plain_date_time(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (ns, tz, cal) = this_zoned(vm, a, "Temporal.ZonedDateTime.prototype.toPlainDateTime")?;
    let dt = zone::date_time_for(vm, &tz, ns);
    create_date_time(vm, dt, cal, None)
}
