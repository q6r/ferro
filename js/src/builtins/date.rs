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

fn parts(t: f64) -> Parts {
    let days = (t / MS_PER_DAY).floor();
    let within = t - days * MS_PER_DAY;
    let (year, month, day) = civil_from_days(days as i64);
    let w = within as i64;
    Parts {
        year,
        month: month - 1,
        day,
        weekday: ((days as i64 + 4).rem_euclid(7)) as usize,
        hour: w / 3_600_000,
        minute: (w / 60_000) % 60,
        second: (w / 1000) % 60,
        ms: w % 1000,
    }
}

/// (ano, mês 0-11, dia, dia da semana, hora, minuto, segundo, ms).
pub fn parts_of(t: f64) -> (i64, i64, i64, usize, i64, i64, i64, i64) {
    let p = parts(t);
    (p.year, p.month, p.day, p.weekday, p.hour, p.minute, p.second, p.ms)
}

/// Deslocamento do fuso local em ms (local = utc + deslocamento).
fn offset_ms(vm: &Vm, t: f64) -> f64 {
    if !t.is_finite() {
        return 0.0;
    }
    (vm.tz)(None, t).map_or(0.0, |z| z.offset_min * 60_000.0)
}

/// Hora local → UTC (considerando a mudança de horário de verão).
pub fn local_to_utc(vm: &Vm, local: f64) -> f64 {
    if !local.is_finite() {
        return local;
    }
    let first = offset_ms(vm, local);
    let second = offset_ms(vm, local - first);
    local - second
}

/// "GMT-0300 (Horário Padrão de Brasília)".
fn tz_suffix(vm: &Vm, t: f64) -> String {
    let z = (vm.tz)(None, t).unwrap_or(crate::vm::TzInfo {
        offset_min: 0.0,
        short: "UTC".into(),
        long: "Coordinated Universal Time".into(),
    });
    let m = z.offset_min as i64;
    let sign = if m < 0 { '-' } else { '+' };
    format!("GMT{sign}{:02}{:02} ({})", m.abs() / 60, m.abs() % 60, z.long)
}

pub fn iso_string(t: f64) -> String {
    let p = parts(t);
    let year = if (0..=9999).contains(&p.year) {
        format!("{:04}", p.year)
    } else if p.year < 0 {
        format!("-{:06}", -p.year)
    } else {
        format!("+{:06}", p.year)
    };
    format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        p.month + 1,
        p.day,
        p.hour,
        p.minute,
        p.second,
        p.ms
    )
}

fn year_str(y: i64) -> String {
    if y < 0 { format!("-{:06}", -y) } else { format!("{y:04}") }
}

fn date_string(t: f64) -> String {
    let p = parts(t);
    format!("{} {} {:02} {}", DAYS[p.weekday], MONTHS[p.month as usize], p.day, year_str(p.year))
}

fn clock_string(t: f64) -> String {
    let p = parts(t);
    format!("{:02}:{:02}:{:02}", p.hour, p.minute, p.second)
}

/// Date.prototype.toString (hora local).
pub fn full_string(vm: &Vm, t: f64) -> String {
    let local = t + offset_ms(vm, t);
    format!("{} {} {}", date_string(local), clock_string(local), tz_suffix(vm, t))
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// Interpreta datas ISO 8601 e os formatos comuns ("Jan 2, 2024 10:00").
/// Interpreta e devolve (tempo, é hora local — falta converter para UTC).
pub fn parse_date_raw(s: &str) -> (f64, bool) {
    let s = s.trim();
    if let Some(r) = parse_iso(s) {
        return r;
    }
    parse_loose(s).unwrap_or((f64::NAN, false))
}

pub fn parse_date(s: &str) -> f64 {
    parse_date_raw(s).0
}

fn parse_date_vm(vm: &Vm, s: &str) -> f64 {
    let (t, local) = parse_date_raw(s);
    time_clip(if local { local_to_utc(vm, t) } else { t })
}

fn parse_iso(s: &str) -> Option<(f64, bool)> {
    let b = s.as_bytes();
    let mut i = 0;
    let num = |i: &mut usize, n: usize| -> Option<i64> {
        if *i + n > b.len() || !b[*i..*i + n].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let v = std::str::from_utf8(&b[*i..*i + n]).ok()?.parse().ok()?;
        *i += n;
        Some(v)
    };
    let year = if b.first() == Some(&b'+') || b.first() == Some(&b'-') {
        let neg = b[0] == b'-';
        i = 1;
        let y = num(&mut i, 6)?;
        if neg { -y } else { y }
    } else {
        num(&mut i, 4)?
    };
    let (mut month, mut day) = (1, 1);
    if i < b.len() && b[i] == b'-' {
        i += 1;
        month = num(&mut i, 2)?;
        if i < b.len() && b[i] == b'-' {
            i += 1;
            day = num(&mut i, 2)?;
        }
    }
    let (mut h, mut mi, mut sec, mut ms) = (0, 0, 0, 0);
    let mut offset: Option<i64> = None;
    let date_only = i == b.len();
    if i < b.len() {
        if b[i] != b'T' && b[i] != b't' && b[i] != b' ' {
            return None;
        }
        i += 1;
        h = num(&mut i, 2)?;
        if i >= b.len() || b[i] != b':' {
            return None;
        }
        i += 1;
        mi = num(&mut i, 2)?;
        if i < b.len() && b[i] == b':' {
            i += 1;
            sec = num(&mut i, 2)?;
            if i < b.len() && (b[i] == b'.' || b[i] == b',') {
                i += 1;
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let frac = &s[start..i];
                let frac = format!("{:0<3}", &frac[..frac.len().min(3)]);
                ms = frac.parse().ok()?;
            }
        }
        if i < b.len() {
            match b[i] {
                b'Z' | b'z' => {
                    offset = Some(0);
                    i += 1;
                }
                b'+' | b'-' => {
                    let sign = if b[i] == b'-' { -1 } else { 1 };
                    i += 1;
                    let oh = num(&mut i, 2)?;
                    if i < b.len() && b[i] == b':' {
                        i += 1;
                    }
                    let om = num(&mut i, 2).unwrap_or(0);
                    offset = Some(sign * (oh * 60 + om));
                }
                _ => return None,
            }
        }
    }
    if i != b.len() || !(1..=12).contains(&month) || !(1..=31).contains(&day) || h > 24 || mi > 59 || sec > 59 {
        return None;
    }
    let d = make_day(year as f64, (month - 1) as f64, day as f64);
    let t = d * MS_PER_DAY + make_time(h as f64, mi as f64, sec as f64, ms as f64);
    let t = t - offset.unwrap_or(0) as f64 * 60_000.0;
    // só data = UTC; data e hora sem fuso = hora local
    Some((t, !date_only && offset.is_none()))
}

