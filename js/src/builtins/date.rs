//! Date. Por enquanto o fuso local é UTC (o M3.4 traz fusos de verdade).

use super::Realm;
use crate::value::*;
use crate::vm::{Hint, Vm};

const MS_PER_DAY: f64 = 86_400_000.0;
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

pub fn install(r: &Realm) {
    let p = &r.date_proto;
    let c = r.ctor("Date", 7, date_ctor, p);
    r.method(&c, "now", 0, now);
    r.method(&c, "parse", 1, parse);
    r.method(&c, "UTC", 7, utc);
    let getters: &[(&str, NativeFn)] = &[
        ("getTime", get_time),
        ("valueOf", get_time),
        ("getFullYear", get_full_year),
        ("getUTCFullYear", get_utc_full_year),
        ("getYear", get_year),
        ("getMonth", get_month),
        ("getUTCMonth", get_utc_month),
        ("getDate", get_date),
        ("getUTCDate", get_utc_date),
        ("getDay", get_day),
        ("getUTCDay", get_utc_day),
        ("getHours", get_hours),
        ("getUTCHours", get_utc_hours),
        ("getMinutes", get_minutes),
        ("getUTCMinutes", get_utc_minutes),
        ("getSeconds", get_seconds),
        ("getUTCSeconds", get_utc_seconds),
        ("getMilliseconds", get_ms),
        ("getUTCMilliseconds", get_utc_ms),
        ("getTimezoneOffset", get_tz_offset),
        ("toISOString", to_iso_string),
        ("toJSON", to_json),
        ("toString", to_string),
        ("toDateString", to_date_string),
        ("toTimeString", to_time_string),
        ("toUTCString", to_utc_string),
        ("toGMTString", to_utc_string),
        ("toLocaleString", to_locale_string),
        ("toLocaleDateString", to_locale_date_string),
        ("toLocaleTimeString", to_locale_time_string),
    ];
    for (name, f) in getters {
        r.method(p, name, 0, *f);
    }
    let setters: &[(&str, u32, NativeFn)] = &[
        ("setTime", 1, set_time),
        ("setMilliseconds", 1, set_ms),
        ("setUTCMilliseconds", 1, set_utc_ms),
        ("setSeconds", 2, set_seconds),
        ("setUTCSeconds", 2, set_utc_seconds),
        ("setMinutes", 3, set_minutes),
        ("setUTCMinutes", 3, set_utc_minutes),
        ("setHours", 4, set_hours),
        ("setUTCHours", 4, set_utc_hours),
        ("setDate", 1, set_date),
        ("setUTCDate", 1, set_utc_date),
        ("setMonth", 2, set_month),
        ("setUTCMonth", 2, set_utc_month),
        ("setFullYear", 3, set_full_year),
        ("setUTCFullYear", 3, set_utc_full_year),
        ("setYear", 1, set_year),
    ];
    for (name, len, f) in setters {
        r.method(p, name, *len, *f);
    }
    r.method_sym(p, &r.sym_to_primitive, "[Symbol.toPrimitive]", 1, to_primitive);
}

// ---------------------------------------------------------- cálculo

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !year.is_finite() || !month.is_finite() || !date.is_finite() {
        return f64::NAN;
    }
    let y = year.trunc() + (month.trunc() / 12.0).floor();
    let m = month.trunc().rem_euclid(12.0);
    if y.abs() > 400_000.0 {
        return f64::NAN;
    }
    days_from_civil(y as i64, m as i64 + 1, 1) as f64 + date.trunc() - 1.0
}

pub fn make_time(h: f64, m: f64, s: f64, ms: f64) -> f64 {
    if !h.is_finite() || !m.is_finite() || !s.is_finite() || !ms.is_finite() {
        return f64::NAN;
    }
    h.trunc() * 3_600_000.0 + m.trunc() * 60_000.0 + s.trunc() * 1000.0 + ms.trunc()
}

pub fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > 8.64e15 {
        return f64::NAN;
    }
    t.trunc() + 0.0
}

struct Parts {
    year: i64,
    month: i64,
    day: i64,
    weekday: usize,
    hour: i64,
    minute: i64,
    second: i64,
    ms: i64,
}

