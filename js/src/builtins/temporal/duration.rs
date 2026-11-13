//! Temporal.Duration e as operações de duração (relativeTo, round, total).

use super::calendar::{self, Cal, DateDur, F_OFFSET, F_TIME_ZONE, Required};
use super::iso::{self, IsoDate, MIDNIGHT, NS_PER_DAY};
use super::options::{self, Group, Opts, UnitOpt};
use super::round::{self, InternalDur, RoundingMode, UNITS, Unit, larger};
use super::zone::{self, Disambig, OffsetBehaviour, OffsetOption, Tz};
use super::{TemporalData, class, create_duration, range, this_duration, to_int_integral, type_err};
use crate::builtins::Realm;
use crate::value::*;
use crate::vm::Vm;

const NAMES: [&str; 10] = [
    "years",
    "months",
    "weeks",
    "days",
    "hours",
    "minutes",
    "seconds",
    "milliseconds",
    "microseconds",
    "nanoseconds",
];

pub fn install(r: &Realm, ns: &ObjRef, p: &ObjRef) {
    let c = class(r, ns, "Duration", 0, ctor, p);
    r.method(&c, "from", 1, from);
    r.method(&c, "compare", 2, compare);
    let getters: [(&str, NativeFn); 12] = [
        ("years", get_years),
        ("months", get_months),
        ("weeks", get_weeks),
        ("days", get_days),
        ("hours", get_hours),
        ("minutes", get_minutes),
        ("seconds", get_seconds),
        ("milliseconds", get_ms),
        ("microseconds", get_us),
        ("nanoseconds", get_ns),
        ("sign", get_sign),
        ("blank", get_blank),
    ];
    for (name, f) in getters {
        r.getter(p, name, f);
    }
    r.method(p, "with", 1, with);
    r.method(p, "negated", 0, negated);
    r.method(p, "abs", 0, abs);
    r.method(p, "add", 1, add);
    r.method(p, "subtract", 1, subtract);
    r.method(p, "round", 1, round);
    r.method(p, "total", 1, total);
    r.method(p, "toString", 0, to_string);
    r.method(p, "toJSON", 0, to_json);
    r.method(p, "toLocaleString", 0, to_locale_string);
    r.method(p, "valueOf", 0, value_of);
}

// --------------------------------------------------------- operações

/// IsValidDuration (em matemática exata).
pub fn is_valid(f: &[f64; 10]) -> bool {
    let mut sign = 0;
    for &v in f {
        if !v.is_finite() {
            return false;
        }
        if v < 0.0 {
            if sign > 0 {
                return false;
            }
            sign = -1;
        } else if v > 0.0 {
            if sign < 0 {
                return false;
            }
            sign = 1;
        }
    }
    const TWO32: f64 = 4294967296.0;
    if f[0].abs() >= TWO32 || f[1].abs() >= TWO32 || f[2].abs() >= TWO32 {
        return false;
    }
    // todos com o mesmo sinal: um termo grande demais já invalida
    const SECS: [f64; 7] = [86400.0, 3600.0, 60.0, 1.0, 1e-3, 1e-6, 1e-9];
    for (k, s) in SECS.iter().enumerate() {
        if f[3 + k].abs() * s > 2.0 * 9007199254740992.0 {
            return false;
        }
    }
    let total = time_from_components(f, true);
    total.abs() < 9_007_199_254_740_992_000_000_000
}

/// Soma exata (ns) dos campos de tempo (e dos dias, se `with_days`).
fn time_from_components(f: &[f64; 10], with_days: bool) -> i128 {
    let i = |x: f64| x as i128;
    let days = if with_days { i(f[3]) * NS_PER_DAY } else { 0 };
    days + i(f[4]) * 3_600_000_000_000 + i(f[5]) * 60_000_000_000 + i(f[6]) * 1_000_000_000 + i(f[7]) * 1_000_000 + i(f[8]) * 1000 + i(f[9])
}

/// DurationSign.
pub fn sign(f: &[f64; 10]) -> i32 {
    for &v in f {
        if v != 0.0 {
            return if v < 0.0 { -1 } else { 1 };
        }
    }
    0
}

/// DefaultTemporalLargestUnit.
pub fn default_largest(f: &[f64; 10]) -> Unit {
    for (k, &v) in f.iter().enumerate() {
        if v != 0.0 {
            return UNITS[k];
        }
    }
    Unit::Nanosecond
}

fn date_part(f: &[f64; 10], days: bool) -> DateDur {
    DateDur {
        years: f[0] as i64,
        months: f[1] as i64,
        weeks: f[2] as i64,
        days: if days { f[3] as i64 } else { 0 },
    }
}

/// ToInternalDurationRecord.
pub fn to_internal(f: &[f64; 10]) -> InternalDur {
    InternalDur {
        date: date_part(f, true),
        time: time_from_components(f, false),
    }
}

/// ToInternalDurationRecordWith24HourDays.
pub fn to_internal_24h(f: &[f64; 10]) -> InternalDur {
    InternalDur {
        date: date_part(f, false),
        time: time_from_components(f, true),
    }
}

/// TemporalDurationFromInternal (sem validar; quem cria valida).
pub fn from_internal(d: &InternalDur, largest: Unit) -> [f64; 10] {
    let sign = d.time.signum();
    let t = d.time.abs();
    let (days, hours, minutes, seconds, ms, us, ns);
    match largest {
        Unit::Year | Unit::Month | Unit::Week | Unit::Day => {
            days = t / NS_PER_DAY;
            hours = t / 3_600_000_000_000 % 24;
            minutes = t / 60_000_000_000 % 60;
            seconds = t / 1_000_000_000 % 60;
            ms = t / 1_000_000 % 1000;
            us = t / 1000 % 1000;
            ns = t % 1000;
        }
        Unit::Hour => {
            days = 0;
            hours = t / 3_600_000_000_000;
            minutes = t / 60_000_000_000 % 60;
            seconds = t / 1_000_000_000 % 60;
            ms = t / 1_000_000 % 1000;
            us = t / 1000 % 1000;
            ns = t % 1000;
        }
        Unit::Minute => {
            (days, hours) = (0, 0);
            minutes = t / 60_000_000_000;
            seconds = t / 1_000_000_000 % 60;
            ms = t / 1_000_000 % 1000;
            us = t / 1000 % 1000;
            ns = t % 1000;
        }
        Unit::Second => {
            (days, hours, minutes) = (0, 0, 0);
            seconds = t / 1_000_000_000;
            ms = t / 1_000_000 % 1000;
            us = t / 1000 % 1000;
            ns = t % 1000;
        }
        Unit::Millisecond => {
            (days, hours, minutes, seconds) = (0, 0, 0, 0);
            ms = t / 1_000_000;
            us = t / 1000 % 1000;
            ns = t % 1000;
        }
        Unit::Microsecond => {
            (days, hours, minutes, seconds, ms) = (0, 0, 0, 0, 0);
            us = t / 1000;
            ns = t % 1000;
        }
        Unit::Nanosecond => {
            (days, hours, minutes, seconds, ms, us) = (0, 0, 0, 0, 0, 0);
            ns = t;
        }
    }
    let v = |x: i128| (sign * x) as f64 + 0.0;
    [
        d.date.years as f64,
        d.date.months as f64,
        d.date.weeks as f64,
        (d.date.days as i128 + sign * days) as f64,
        v(hours),
        v(minutes),
        v(seconds),
        v(ms),
        v(us),
        v(ns),
    ]
}

/// TemporalDurationFromInternal + CreateTemporalDuration.
pub fn create_from_internal(vm: &mut Vm, d: &InternalDur, largest: Unit) -> Result<Value, Value> {
    create_duration(vm, from_internal(d, largest), None)
}

pub fn negate(f: &[f64; 10]) -> [f64; 10] {
    f.map(|x| -x + 0.0)
}

/// ToTemporalPartialDurationRecord.
fn to_partial(vm: &mut Vm, v: &Value) -> Result<[Option<f64>; 10], Value> {
    if !matches!(v, Value::Object(_)) {
        let d = vm.describe(v);
        return type_err(vm, format!("Invalid duration-like value: {d}"));
    }
    // ordem alfabética das propriedades
    const ORDER: [usize; 10] = [3, 4, 8, 7, 5, 1, 9, 6, 2, 0];
    let mut out = [None; 10];
    let mut any = false;
    for k in ORDER {
        let x = vm.get_str(v, NAMES[k])?;
        if !x.is_undefined() {
            out[k] = Some(to_int_integral(vm, &x, NAMES[k])?);
            any = true;
        }
    }
    if !any {
        return type_err(vm, "Invalid duration-like value: no duration properties");
    }
    Ok(out)
}

/// ToTemporalDuration → campos válidos.
pub fn to_duration(vm: &mut Vm, v: &Value) -> Result<[f64; 10], Value> {
    if let Some(TemporalData::Duration(f)) = super::data(v) {
        return Ok(f);
    }
    let f = match v {
        Value::Object(_) => to_partial(vm, v)?.map(|x| x.unwrap_or(0.0)),
        Value::String(s) => {
            let s = s.to_rust();
            match super::parse::parse_duration(&s) {
                Some(f) => f,
                None => return range(vm, format!("Invalid duration string: {s}")),
            }
        }
        _ => {
            let d = vm.describe(v);
            return type_err(vm, format!("Invalid duration: {d}"));
        }
    };
    if !is_valid(&f) {
        return range(vm, "Invalid duration");
    }
    Ok(f)
}

/// DateDurationDays.
fn date_duration_days(vm: &mut Vm, d: &DateDur, rel: &IsoDate, cal: Cal) -> Result<i64, Value> {
    let ymw = round::adjust(vm, d, 0, None, None)?;
    if ymw.sign() == 0 {
        return Ok(d.days);
    }
    let later = calendar::date_add(vm, cal, rel, &ymw, true)?;
    Ok(d.days + iso::date_epoch_days(&later) - iso::date_epoch_days(rel))
}

/// O relativeTo de round/total/compare.
pub enum Relative {
    None,
    Plain(IsoDate, Cal),
    Zoned(i128, Tz, Cal),
}

/// GetTemporalRelativeToOption.
pub fn relative_to(vm: &mut Vm, o: &Opts) -> Result<Relative, Value> {
    let v = options::get(vm, o, "relativeTo")?;
    if v.is_undefined() {
        return Ok(Relative::None);
    }
    let mut behaviour = OffsetBehaviour::Option;
    let mut match_minutes = false;
    let (date, time, tz, offset, cal);
    if let Value::Object(_) = &v {
        match super::data(&v) {
            Some(TemporalData::Zoned(ns, tz, c)) => return Ok(Relative::Zoned(ns, tz, c)),
            Some(TemporalData::Date(d, c)) => return Ok(Relative::Plain(d, c)),
            Some(TemporalData::DateTime(dt, c)) => return Ok(Relative::Plain(dt.date, c)),
            _ => {}
        }
        cal = calendar::calendar_with_iso_default(vm, &v)?;
        let names = calendar::DATE_FIELDS | calendar::TIME_FIELDS | F_OFFSET | F_TIME_ZONE;
        let f = calendar::prepare(vm, cal, &v, names, Required::None)?;
        let dt = calendar::interpret_date_time(vm, cal, &f, true)?;
        date = dt.date;
        time = Some(dt.time);
        tz = f.time_zone;
        offset = f.offset;
        if offset.is_none() {
            behaviour = OffsetBehaviour::Wall;
        }
    } else {
        let Value::String(s) = &v else {
            let d = vm.describe(&v);
            return type_err(vm, format!("Invalid relativeTo: {d}"));
        };
        let s = s.to_rust();
        let p = match super::parse::parse_iso(&s, &[super::parse::Goal::Zoned, super::parse::Goal::DateTime]) {
            Ok(p) => p,
            Err(e) => return range(vm, e),
        };
        offset = p.offset.clone();
        tz = match &p.tz {
            None => None,
            Some(a) => {
                let t = zone::from_string(vm, a)?;
                if p.z {
                    behaviour = OffsetBehaviour::Exact;
                } else if offset.is_none() {
                    behaviour = OffsetBehaviour::Wall;
                }
                match_minutes = !offset.as_deref().is_some_and(super::parse::offset_has_seconds);
                Some(t)
            }
        };
        let id = p.calendar.clone().unwrap_or_else(|| "iso8601".into());
        cal = calendar::canonicalize_or_throw(vm, &id)?;
        date = IsoDate::new(p.year.unwrap_or(0), p.month, p.day);
        time = p.time;
    }
    let Some(tz) = tz else {
        if !iso::date_within_limits(&date) {
            return range(vm, "Date is outside of supported range");
        }
        return Ok(Relative::Plain(date, cal));
    };
    let off = match (behaviour, &offset) {
        (OffsetBehaviour::Option, Some(o)) => super::parse::parse_utc_offset(o).unwrap_or(0),
        _ => 0,
    };
    let ns = zone::interpret_offset(vm, date, time, behaviour, off, &tz, Disambig::Compatible, OffsetOption::Reject, match_minutes)?;
    Ok(Relative::Zoned(ns, tz, cal))
}

/// TemporalDurationToString.
pub fn to_iso_string(f: &[f64; 10], precision: Option<u8>) -> String {
    let num = |x: f64| (x.abs() as i128).to_string();
    let mut date = String::new();
    for (k, d) in ["Y", "M", "W", "D"].iter().enumerate() {
        if f[k] != 0.0 {
            date.push_str(&num(f[k]));
            date.push_str(d);
        }
    }
    let mut time = String::new();
    if f[4] != 0.0 {
        time.push_str(&num(f[4]));
        time.push('H');
    }
    if f[5] != 0.0 {
        time.push_str(&num(f[5]));
        time.push('M');
    }
    let secs = (f[6] as i128) * 1_000_000_000 + (f[7] as i128) * 1_000_000 + (f[8] as i128) * 1000 + f[9] as i128;
    if secs != 0 || default_largest(f) >= Unit::Second || precision.is_some() {
        let a = secs.abs();
        time.push_str(&(a / 1_000_000_000).to_string());
        time.push_str(&iso::fraction_string((a % 1_000_000_000) as u32, precision));
        time.push('S');
    }
    let mut out = String::new();
    if sign(f) < 0 {
        out.push('-');
    }
    out.push('P');
    out.push_str(&date);
    if !time.is_empty() {
        out.push('T');
        out.push_str(&time);
    }
    out
}

// ------------------------------------------------------------ nativos

fn ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return type_err(vm, "Constructor Temporal.Duration requires 'new'");
    };
    let mut f = [0.0; 10];
    for (k, x) in f.iter_mut().enumerate() {
        let v = a.arg(k);
        if !v.is_undefined() {
            *x = to_int_integral(vm, &v, NAMES[k])?;
        }
    }
    create_duration(vm, f, Some(&nt))
}

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = to_duration(vm, &a.arg(0))?;
    create_duration(vm, f, None)
}

fn compare(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let one = to_duration(vm, &a.arg(0))?;
    let two = to_duration(vm, &a.arg(1))?;
    let o = options::get_options(vm, &a.arg(2))?;
    let rel = relative_to(vm, &o)?;
    if one == two {
        return Ok(Value::Number(0.0));
    }
    let (l1, l2) = (default_largest(&one), default_largest(&two));
    let (d1, d2) = (to_internal(&one), to_internal(&two));
    if let Relative::Zoned(ns, tz, cal) = &rel
        && (l1.is_date() || l2.is_date())
    {
        let a1 = round::add_zoned(vm, *ns, tz, *cal, &d1, true)?;
        let a2 = round::add_zoned(vm, *ns, tz, *cal, &d2, true)?;
        return Ok(Value::Number(iso::ord(a1.cmp(&a2)) as f64));
    }
    let (mut days1, mut days2) = (d1.date.days, d2.date.days);
    if l1.is_calendar() || l2.is_calendar() {
        let Relative::Plain(date, cal) = rel else {
            return range(vm, "A starting point is required for years, months, or weeks comparison");
        };
        days1 = date_duration_days(vm, &d1.date, &date, cal)?;
        days2 = date_duration_days(vm, &d2.date, &date, cal)?;
    }
    let t1 = round::check_time(vm, d1.time + days1 as i128 * NS_PER_DAY)?;
    let t2 = round::check_time(vm, d2.time + days2 as i128 * NS_PER_DAY)?;
    Ok(Value::Number(iso::ord(t1.cmp(&t2)) as f64))
}

macro_rules! field_getter {
    ($name:ident, $k:expr, $js:expr) => {
        fn $name(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
            let f = this_duration(vm, a, concat!("Temporal.Duration.prototype.", $js))?;
            Ok(Value::Number(f[$k]))
        }
    };
}

field_getter!(get_years, 0, "years");
field_getter!(get_months, 1, "months");
field_getter!(get_weeks, 2, "weeks");
field_getter!(get_days, 3, "days");
field_getter!(get_hours, 4, "hours");
field_getter!(get_minutes, 5, "minutes");
field_getter!(get_seconds, 6, "seconds");
field_getter!(get_ms, 7, "milliseconds");
field_getter!(get_us, 8, "microseconds");
field_getter!(get_ns, 9, "nanoseconds");

fn get_sign(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.sign")?;
    Ok(Value::Number(sign(&f) as f64))
}

fn get_blank(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.blank")?;
    Ok(Value::Bool(sign(&f) == 0))
}

fn with(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.with")?;
    let p = to_partial(vm, &a.arg(0))?;
    let mut out = f;
    for k in 0..10 {
        if let Some(v) = p[k] {
            out[k] = v;
        }
    }
    create_duration(vm, out, None)
}

fn negated(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.negated")?;
    create_duration(vm, negate(&f), None)
}

fn abs(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.abs")?;
    create_duration(vm, f.map(f64::abs), None)
}

/// AddDurations.
fn add_durations(vm: &mut Vm, a: &CallArgs, sub: bool, method: &str) -> Result<Value, Value> {
    let f = this_duration(vm, a, method)?;
    let mut other = to_duration(vm, &a.arg(0))?;
    if sub {
        other = negate(&other);
    }
    let largest = larger(default_largest(&f), default_largest(&other));
    if largest.is_calendar() {
        return range(vm, "For years, months, or weeks arithmetic, use date arithmetic relative to a starting point");
    }
    let t = round::check_time(vm, to_internal_24h(&f).time + to_internal_24h(&other).time)?;
    create_from_internal(vm, &InternalDur { date: DateDur::default(), time: t }, largest)
}

fn add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_durations(vm, a, false, "Temporal.Duration.prototype.add")
}

fn subtract(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_durations(vm, a, true, "Temporal.Duration.prototype.subtract")
}

/// Opções de round/total: string vira { chave: string }.
fn options_or_string(vm: &mut Vm, v: &Value, key: &str) -> Result<Opts, Value> {
    if v.is_undefined() {
        return type_err(vm, "options parameter is required");
    }
    if let Value::String(_) = v {
        return Ok(Some(super::options_with(key, v.clone())));
    }
    options::get_options(vm, v)
}

fn round(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.round")?;
    let o = options_or_string(vm, &a.arg(0), "smallestUnit")?;
    let existing = default_largest(&f);
    let largest_opt = options::unit(vm, &o, "largestUnit", false)?;
    let rel = relative_to(vm, &o)?;
    let inc = options::rounding_increment(vm, &o)?;
    let mode = options::rounding_mode(vm, &o, RoundingMode::HalfExpand)?;
    let smallest_opt = options::unit(vm, &o, "smallestUnit", false)?;
    options::validate_unit(vm, smallest_opt, Group::DateTime, &[])?;
    let smallest = match smallest_opt {
        UnitOpt::U(u) => u,
        _ => Unit::Nanosecond,
    };
    let default_l = larger(existing, smallest);
    let largest = match largest_opt {
        UnitOpt::U(u) => u,
        _ => default_l,
    };
    if smallest_opt == UnitOpt::Unset && largest_opt == UnitOpt::Unset {
        return range(vm, "at least one of smallestUnit or largestUnit is required");
    }
    if larger(largest, smallest) != largest {
        return range(vm, format!("largestUnit {} cannot be smaller than smallestUnit {}", largest.singular(), smallest.singular()));
    }
    if let Some(max) = smallest.max_increment() {
        options::validate_increment(vm, inc, max, false)?;
    }
    if inc > 1 && smallest.is_date() && largest != smallest {
        return range(vm, "For calendar units with roundingIncrement > 1, use largestUnit = smallestUnit");
    }
    match rel {
        Relative::Zoned(ns, tz, cal) => {
            let d = to_internal(&f);
            let target = round::add_zoned(vm, ns, &tz, cal, &d, true)?;
            let r = round::diff_zoned_rounded(vm, ns, target, &tz, cal, largest, inc, smallest, mode)?;
            let lu = if largest.is_date() { Unit::Hour } else { largest };
            create_from_internal(vm, &r, lu)
        }
        Relative::Plain(date, cal) => {
            let d = to_internal_24h(&f);
            let (days, time) = iso::add_time(&MIDNIGHT, d.time);
            let dd = round::adjust(vm, &d.date, days, None, None)?;
            let target = calendar::date_add(vm, cal, &date, &dd, true)?;
            let r = round::diff_date_time_rounded(vm, &date.at(MIDNIGHT), &target.at(time), cal, largest, inc, smallest, mode)?;
            create_from_internal(vm, &r, largest)
        }
        Relative::None => {
            if existing.is_calendar() || largest.is_calendar() {
                let u = if existing.is_calendar() { existing } else { largest };
                return range(vm, format!("A starting point is required for {}s balancing", u.singular()));
            }
            let d = to_internal_24h(&f);
            let r = if smallest == Unit::Day {
                let t = round::round_time_dur(vm, d.time, inc, Unit::Day, mode)?;
                InternalDur {
                    date: DateDur {
                        days: (t / NS_PER_DAY) as i64,
                        ..DateDur::default()
                    },
                    time: 0,
                }
            } else {
                InternalDur {
                    date: DateDur::default(),
                    time: round::round_time_dur(vm, d.time, inc, smallest, mode)?,
                }
            };
            create_from_internal(vm, &r, largest)
        }
    }
}

fn total(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.total")?;
    let o = options_or_string(vm, &a.arg(0), "unit")?;
    let rel = relative_to(vm, &o)?;
    let unit_opt = options::unit(vm, &o, "unit", true)?;
    options::validate_unit(vm, unit_opt, Group::DateTime, &[])?;
    let UnitOpt::U(unit) = unit_opt else {
        return range(vm, "unit option is required");
    };
    let t = match rel {
        Relative::Zoned(ns, tz, cal) => {
            let d = to_internal(&f);
            let target = round::add_zoned(vm, ns, &tz, cal, &d, true)?;
            round::diff_zoned_total(vm, ns, target, &tz, cal, unit)?
        }
        Relative::Plain(date, cal) => {
            let d = to_internal_24h(&f);
            let (days, time) = iso::add_time(&MIDNIGHT, d.time);
            let dd = round::adjust(vm, &d.date, days, None, None)?;
            let target = calendar::date_add(vm, cal, &date, &dd, true)?;
            round::diff_date_time_total(vm, &date.at(MIDNIGHT), &target.at(time), cal, unit)?
        }
        Relative::None => {
            let largest = default_largest(&f);
            if largest.is_calendar() || unit.is_calendar() {
                let u = if largest.is_calendar() { largest } else { unit };
                return range(vm, format!("A starting point is required for {}s total", u.singular()));
            }
            round::fdiv(to_internal_24h(&f).time, unit.ns())
        }
    };
    Ok(Value::Number(t))
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.toString")?;
    let o = options::get_options(vm, &a.arg(0))?;
    let digits = options::fractional_digits(vm, &o)?;
    let mode = options::rounding_mode(vm, &o, RoundingMode::Trunc)?;
    let smallest = options::unit(vm, &o, "smallestUnit", false)?;
    options::validate_unit(vm, smallest, Group::Time, &[])?;
    if smallest == UnitOpt::U(Unit::Hour) || smallest == UnitOpt::U(Unit::Minute) {
        return range(vm, "smallestUnit must be a time unit other than \"hours\" or \"minutes\"");
    }
    let (precision, unit, inc) = options::seconds_precision(smallest, digits);
    if unit == Unit::Nanosecond && inc == 1 {
        return Ok(Value::str(&to_iso_string(&f, precision)));
    }
    let largest = default_largest(&f);
    let d = to_internal(&f);
    let t = round::round_time_dur(vm, d.time, inc, unit, mode)?;
    let r = from_internal(&InternalDur { date: d.date, time: t }, larger(largest, Unit::Second));
    if !is_valid(&r) {
        return range(vm, "Invalid duration");
    }
    Ok(Value::str(&to_iso_string(&r, precision)))
}

fn to_json(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.toJSON")?;
    Ok(Value::str(&to_iso_string(&f, None)))
}

fn to_locale_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = this_duration(vm, a, "Temporal.Duration.prototype.toLocaleString")?;
    Ok(Value::str(&to_iso_string(&f, None)))
}

fn value_of(vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    super::value_of_throws(vm, "Duration")
}
