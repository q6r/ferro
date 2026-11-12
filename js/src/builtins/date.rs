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

