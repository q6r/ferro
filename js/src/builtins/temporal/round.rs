//! Unidades, modos de arredondamento, durações internas e as diferenças
//! (com arredondamento relativo a uma data ou fuso) entre datas e instantes.

use std::cmp::Ordering;

use super::calendar::{self, Cal, DateDur};
use super::iso::{self, DateTime, MAX_TIME_DURATION, NS_PER_DAY, Time};
use super::range;
use super::zone::{self, Disambig, Tz};
use crate::value::Value;
use crate::vm::Vm;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Unit {
    Year,
    Month,
    Week,
    Day,
    Hour,
    Minute,
    Second,
    Millisecond,
    Microsecond,
    Nanosecond,
}

pub const UNITS: [Unit; 10] = [
    Unit::Year,
    Unit::Month,
    Unit::Week,
    Unit::Day,
    Unit::Hour,
    Unit::Minute,
    Unit::Second,
    Unit::Millisecond,
    Unit::Microsecond,
    Unit::Nanosecond,
];

impl Unit {
    /// Tamanho em ns (dia e menores).
    pub fn ns(self) -> i128 {
        match self {
            Unit::Nanosecond => 1,
            Unit::Microsecond => 1000,
            Unit::Millisecond => 1_000_000,
            Unit::Second => 1_000_000_000,
            Unit::Minute => 60_000_000_000,
            Unit::Hour => 3_600_000_000_000,
            _ => NS_PER_DAY,
        }
    }

    pub fn is_calendar(self) -> bool {
        matches!(self, Unit::Year | Unit::Month | Unit::Week)
    }

    pub fn is_date(self) -> bool {
        self <= Unit::Day
    }

    pub fn singular(self) -> &'static str {
        match self {
            Unit::Year => "year",
            Unit::Month => "month",
            Unit::Week => "week",
            Unit::Day => "day",
            Unit::Hour => "hour",
            Unit::Minute => "minute",
            Unit::Second => "second",
            Unit::Millisecond => "millisecond",
            Unit::Microsecond => "microsecond",
            Unit::Nanosecond => "nanosecond",
        }
    }

    /// MaximumTemporalDurationRoundingIncrement.
    pub fn max_increment(self) -> Option<i64> {
        match self {
            Unit::Hour => Some(24),
            Unit::Minute | Unit::Second => Some(60),
            Unit::Millisecond | Unit::Microsecond | Unit::Nanosecond => Some(1000),
            _ => None,
        }
    }
}

/// LargerOfTwoTemporalUnits.
pub fn larger(a: Unit, b: Unit) -> Unit {
    a.min(b)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoundingMode {
    Ceil,
    Floor,
    Expand,
    Trunc,
    HalfCeil,
    HalfFloor,
    HalfExpand,
    HalfTrunc,
    HalfEven,
}

impl RoundingMode {
    /// NegateRoundingMode.
    pub fn negate(self) -> RoundingMode {
        match self {
            RoundingMode::Ceil => RoundingMode::Floor,
            RoundingMode::Floor => RoundingMode::Ceil,
            RoundingMode::HalfCeil => RoundingMode::HalfFloor,
            RoundingMode::HalfFloor => RoundingMode::HalfCeil,
            m => m,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unsigned {
    Zero,
    Infinity,
    HalfZero,
    HalfInfinity,
    HalfEven,
}

/// GetUnsignedRoundingMode.
fn unsigned(mode: RoundingMode, negative: bool) -> Unsigned {
    use RoundingMode::*;
    match (mode, negative) {
        (Ceil, false) | (Floor, true) | (Expand, _) => Unsigned::Infinity,
        (Ceil, true) | (Floor, false) | (Trunc, _) => Unsigned::Zero,
        (HalfCeil, false) | (HalfFloor, true) | (HalfExpand, _) => Unsigned::HalfInfinity,
        (HalfCeil, true) | (HalfFloor, false) | (HalfTrunc, _) => Unsigned::HalfZero,
        (HalfEven, _) => Unsigned::HalfEven,
    }
}

/// ApplyUnsignedRoundingMode (cmp: 2×resto comparado ao incremento).
fn apply(r1: i128, r2: i128, cmp: Ordering, even: bool, u: Unsigned) -> i128 {
    match u {
        Unsigned::Zero => r1,
        Unsigned::Infinity => r2,
        _ => match cmp {
            Ordering::Less => r1,
            Ordering::Greater => r2,
            Ordering::Equal => match u {
                Unsigned::HalfZero => r1,
                Unsigned::HalfInfinity => r2,
                _ => {
                    if even {
                        r1
                    } else {
                        r2
                    }
                }
            },
        },
    }
}

/// RoundNumberToIncrement (simétrico em torno do zero).
pub fn round_i128(x: i128, inc: i128, mode: RoundingMode) -> i128 {
    let rem = x % inc;
    if rem == 0 {
        return x;
    }
    let neg = x < 0;
    let r1 = (x / inc).abs();
    let cmp = (rem.abs() * 2).cmp(&inc);
    let r = apply(r1, r1 + 1, cmp, r1 % 2 == 0, unsigned(mode, neg));
    (if neg { -r } else { r }) * inc
}

/// RoundNumberToIncrementAsIfPositive (instantes: "para baixo" é para o passado).
pub fn round_positive(x: i128, inc: i128, mode: RoundingMode) -> i128 {
    let rem = x.rem_euclid(inc);
    if rem == 0 {
        return x;
    }
    let r1 = x.div_euclid(inc);
    let cmp = (rem * 2).cmp(&inc);
    apply(r1, r1 + 1, cmp, r1.rem_euclid(2) == 0, unsigned(mode, false)) * inc
}

/// Divisão exata n/d → Number com arredondamento correto.
pub fn fdiv(n: i128, d: i128) -> f64 {
    let neg = (n < 0) != (d < 0) && n != 0;
    let (n, d) = (n.unsigned_abs(), d.unsigned_abs());
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push_str(&(n / d).to_string());
    let mut rem = n % d;
    if rem != 0 {
        s.push('.');
        let mut k = 0;
        while rem != 0 && k < 60 {
            // rem < d ≤ ~1e27: rem * 10 cabe no u128
            rem *= 10;
            s.push((b'0' + (rem / d) as u8) as char);
            rem %= d;
            k += 1;
        }
    }
    s.parse().unwrap_or(f64::NAN)
}

// ------------------------------------------------- durações internas

/// Internal Duration Record: data (anos, meses, semanas, dias) + tempo em ns.
#[derive(Clone, Copy, Debug, Default)]
pub struct InternalDur {
    pub date: DateDur,
    pub time: i128,
}

impl InternalDur {
    pub fn sign(&self) -> i64 {
        let s = self.date.sign();
        if s != 0 { s } else { self.time.signum() as i64 }
    }
}

fn date_dur_valid(d: &DateDur) -> bool {
    super::duration::is_valid(&[d.years as f64, d.months as f64, d.weeks as f64, d.days as f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
}

/// AdjustDateDurationRecord.
pub fn adjust(vm: &mut Vm, d: &DateDur, days: i64, weeks: Option<i64>, months: Option<i64>) -> Result<DateDur, Value> {
    let r = DateDur {
        years: d.years,
        months: months.unwrap_or(d.months),
        weeks: weeks.unwrap_or(d.weeks),
        days,
    };
    if !date_dur_valid(&r) {
        return range(vm, "Invalid duration: date units out of range");
    }
    Ok(r)
}

/// Confere o limite de uma duração de tempo (±2^53 s).
pub fn check_time(vm: &mut Vm, t: i128) -> Result<i128, Value> {
    if t.abs() > MAX_TIME_DURATION {
        return range(vm, "Time duration is outside of supported range");
    }
    Ok(t)
}

/// RoundTimeDuration.
pub fn round_time_dur(vm: &mut Vm, t: i128, inc: i64, unit: Unit, mode: RoundingMode) -> Result<i128, Value> {
    check_time(vm, round_i128(t, unit.ns() * inc as i128, mode))
}

/// AddInstant.
pub fn add_instant(vm: &mut Vm, ns: i128, t: i128) -> Result<i128, Value> {
    let r = ns + t;
    if !iso::is_valid_epoch_ns(r) {
        return range(vm, "Instant is outside of supported range");
    }
    Ok(r)
}

/// RoundTime → (dias a mais, hora).
pub fn round_time(t: &Time, inc: i64, unit: Unit, mode: RoundingMode) -> (i64, Time) {
    let h = t.hour as i128 * 3_600_000_000_000;
    let mi = t.minute as i128 * 60_000_000_000;
    let s = t.second as i128 * 1_000_000_000;
    let ms = t.ms as i128 * 1_000_000;
    let us = t.us as i128 * 1000;
    let ns = t.ns as i128;
    let quantity = match unit {
        Unit::Day | Unit::Hour => h + mi + s + ms + us + ns,
        Unit::Minute => mi + s + ms + us + ns,
        Unit::Second => s + ms + us + ns,
        Unit::Millisecond => ms + us + ns,
        Unit::Microsecond => us + ns,
        _ => ns,
    };
    let len = unit.ns();
    let r = round_i128(quantity, len * inc as i128, mode) / len;
    let (hh, mm, ss, mss, uss) = (t.hour as i128, t.minute as i128, t.second as i128, t.ms as i128, t.us as i128);
    match unit {
        Unit::Day => (r as i64, iso::MIDNIGHT),
        Unit::Hour => iso::balance_time(r, 0, 0, 0, 0, 0),
        Unit::Minute => iso::balance_time(hh, r, 0, 0, 0, 0),
        Unit::Second => iso::balance_time(hh, mm, r, 0, 0, 0),
        Unit::Millisecond => iso::balance_time(hh, mm, ss, r, 0, 0),
        Unit::Microsecond => iso::balance_time(hh, mm, ss, mss, r, 0),
        _ => iso::balance_time(hh, mm, ss, mss, uss, r),
    }
}

/// RoundISODateTime.
pub fn round_date_time(dt: &DateTime, inc: i64, unit: Unit, mode: RoundingMode) -> DateTime {
    let (days, time) = round_time(&dt.time, inc, unit, mode);
    DateTime {
        date: iso::add_days(dt.date, days),
        time,
    }
}

/// DifferenceTime.
pub fn diff_time(a: &Time, b: &Time) -> i128 {
    b.total_ns() as i128 - a.total_ns() as i128
}

/// DifferenceISODateTime.
pub fn diff_date_time(vm: &mut Vm, a: &DateTime, b: &DateTime, cal: Cal, largest: Unit) -> Result<InternalDur, Value> {
    let mut time = diff_time(&a.time, &b.time);
    let time_sign = time.signum() as i32;
    let date_sign = iso::cmp_date(&a.date, &b.date);
    let mut adjusted = b.date;
    if date_sign == time_sign {
        adjusted = iso::add_days(adjusted, time_sign as i64);
        time -= time_sign as i128 * NS_PER_DAY;
    }
    let date_largest = larger(Unit::Day, largest);
    let mut dd = calendar::date_until(cal, &a.date, &adjusted, date_largest);
    if largest != date_largest {
        time = check_time(vm, time + dd.days as i128 * NS_PER_DAY)?;
        dd.days = 0;
    }
    Ok(InternalDur { date: dd, time })
}

/// DifferenceZonedDateTime.
pub fn diff_zoned(vm: &mut Vm, ns1: i128, ns2: i128, tz: &Tz, cal: Cal, largest: Unit) -> Result<InternalDur, Value> {
    let diff = ns2 - ns1;
    if diff == 0 {
        return Ok(InternalDur::default());
    }
    let sign: i64 = if diff < 0 { 1 } else { -1 };
    let start = zone::date_time_for(vm, tz, ns1);
    let end = zone::date_time_for(vm, tz, ns2);
    if start.date == end.date {
        return Ok(InternalDur {
            date: DateDur::default(),
            time: diff,
        });
    }
    let max_correction = if sign == -1 { 2 } else { 1 };
    let mut correction = 0;
    let mut time = diff_time(&start.time, &end.time);
    if time.signum() as i64 == sign {
        correction += 1;
    }
    let mut mid = start;
    while correction <= max_correction {
        let mid_date = iso::add_days(end.date, correction * sign);
        mid = mid_date.at(start.time);
        let mid_ns = zone::epoch_for(vm, tz, &mid, Disambig::Compatible)?;
        time = ns2 - mid_ns;
        if time.signum() as i64 != sign {
            break;
        }
        correction += 1;
    }
    let date_largest = larger(Unit::Day, largest);
    let dd = calendar::date_until(cal, &start.date, &mid.date, date_largest);
    Ok(InternalDur { date: dd, time })
}

struct Window {
    r1: i128,
    r2: i128,
    start_ns: i128,
    end_ns: i128,
    start: DateDur,
    end: DateDur,
}

fn to_epoch(vm: &mut Vm, tz: Option<&Tz>, dt: &DateTime) -> Result<i128, Value> {
    match tz {
        Some(tz) => zone::epoch_for(vm, tz, dt, Disambig::Compatible),
        None => Ok(iso::epoch_ns(dt)),
    }
}

/// ComputeNudgeWindow.
#[allow(clippy::too_many_arguments)]
fn nudge_window(
    vm: &mut Vm,
    sign: i64,
    dur: &InternalDur,
    origin: i128,
    dt: &DateTime,
    tz: Option<&Tz>,
    cal: Cal,
    inc: i64,
    unit: Unit,
    shift: bool,
) -> Result<Window, Value> {
    let step = inc as i128 * sign as i128;
    let (r1, r2, start, end) = match unit {
        Unit::Year => {
            let y = round_i128(dur.date.years as i128, inc as i128, RoundingMode::Trunc);
            let r1 = if shift { y + step } else { y };
            let r2 = r1 + step;
            let z = DateDur::default();
            (r1, r2, DateDur { years: r1 as i64, ..z }, DateDur { years: r2 as i64, ..z })
        }
        Unit::Month => {
            let m = round_i128(dur.date.months as i128, inc as i128, RoundingMode::Trunc);
            let r1 = if shift { m + step } else { m };
            let r2 = r1 + step;
            let s = adjust(vm, &dur.date, 0, Some(0), Some(r1 as i64))?;
            let e = adjust(vm, &dur.date, 0, Some(0), Some(r2 as i64))?;
            (r1, r2, s, e)
        }
        Unit::Week => {
            let ym = adjust(vm, &dur.date, 0, Some(0), None)?;
            let weeks_start = calendar::date_add(vm, cal, &dt.date, &ym, true)?;
            let weeks_end = iso::add_days(weeks_start, dur.date.days);
            let until = calendar::date_until(cal, &weeks_start, &weeks_end, Unit::Week);
            let w = round_i128((dur.date.weeks + until.weeks) as i128, inc as i128, RoundingMode::Trunc);
            let r2 = w + step;
            let s = adjust(vm, &dur.date, 0, Some(w as i64), None)?;
            let e = adjust(vm, &dur.date, 0, Some(r2 as i64), None)?;
            (w, r2, s, e)
        }
        _ => {
            let d = round_i128(dur.date.days as i128, inc as i128, RoundingMode::Trunc);
            let r2 = d + step;
            let s = adjust(vm, &dur.date, d as i64, None, None)?;
            let e = adjust(vm, &dur.date, r2 as i64, None, None)?;
            (d, r2, s, e)
        }
    };
    let start_ns = if start.sign() == 0 {
        origin
    } else {
        let s = calendar::date_add(vm, cal, &dt.date, &start, true)?;
        to_epoch(vm, tz, &s.at(dt.time))?
    };
    let e = calendar::date_add(vm, cal, &dt.date, &end, true)?;
    let end_ns = to_epoch(vm, tz, &e.at(dt.time))?;
    Ok(Window {
        r1,
        r2,
        start_ns,
        end_ns,
        start,
        end,
    })
}

struct Nudge {
    dur: InternalDur,
    nudged_ns: i128,
    expanded: bool,
}

/// NudgeToCalendarUnit → (resultado, total quando o incremento é 1).
#[allow(clippy::too_many_arguments)]
fn nudge_calendar(
    vm: &mut Vm,
    sign: i64,
    dur: &InternalDur,
    origin: i128,
    dest: i128,
    dt: &DateTime,
    tz: Option<&Tz>,
    cal: Cal,
    inc: i64,
    unit: Unit,
    mode: RoundingMode,
) -> Result<(Nudge, f64), Value> {
    let mut expanded = false;
    let mut w = nudge_window(vm, sign, dur, origin, dt, tz, cal, inc, unit, false)?;
    let inside = |w: &Window| {
        if sign == 1 {
            w.start_ns <= dest && dest <= w.end_ns
        } else {
            w.end_ns <= dest && dest <= w.start_ns
        }
    };
    if !inside(&w) {
        w = nudge_window(vm, sign, dur, origin, dt, tz, cal, inc, unit, true)?;
        expanded = true;
    }
    if !inside(&w) || w.end_ns == w.start_ns {
        return range(vm, "Rounding produced an inconsistent duration");
    }
    let num = dest - w.start_ns;
    let den = w.end_ns - w.start_ns;
    let u = unsigned(mode, sign < 0);
    let cmp = (num.abs() * 2).cmp(&den.abs());
    let even = (w.r1.abs() / inc as i128) % 2 == 0;
    let rounded = if num == 0 {
        w.r1.abs()
    } else if num == den {
        w.r2.abs()
    } else {
        apply(w.r1.abs(), w.r2.abs(), cmp, even, u)
    };
    let total = if inc == 1 {
        fdiv(den * w.r1 + num * sign as i128, den)
    } else {
        f64::NAN
    };
    expanded = expanded || rounded == w.r2.abs();
    let date = if rounded == w.r2.abs() { w.end } else { w.start };
    Ok((
        Nudge {
            dur: InternalDur { date, time: 0 },
            nudged_ns: if expanded { w.end_ns } else { w.start_ns },
            expanded,
        },
        total,
    ))
}

/// NudgeToZonedTime.
#[allow(clippy::too_many_arguments)]
fn nudge_zoned(
    vm: &mut Vm,
    sign: i64,
    dur: &InternalDur,
    dt: &DateTime,
    tz: &Tz,
    cal: Cal,
    inc: i64,
    unit: Unit,
    mode: RoundingMode,
) -> Result<Nudge, Value> {
    let start = calendar::date_add(vm, cal, &dt.date, &dur.date, true)?;
    let end = iso::add_days(start, sign);
    let start_ns = zone::epoch_for(vm, tz, &start.at(dt.time), Disambig::Compatible)?;
    let end_ns = zone::epoch_for(vm, tz, &end.at(dt.time), Disambig::Compatible)?;
    let span = end_ns - start_ns;
    if span.signum() as i64 != sign {
        return range(vm, "Time zone returned inconsistent instants");
    }
    let unit_inc = unit.ns() * inc as i128;
    let mut rounded = check_time(vm, round_i128(dur.time, unit_inc, mode))?;
    let beyond = check_time(vm, rounded - span)?;
    let beyond_day = beyond.signum() as i64 != -sign;
    let (delta, nudged) = if beyond_day {
        rounded = check_time(vm, round_i128(beyond, unit_inc, mode))?;
        (sign, end_ns + rounded)
    } else {
        (0, start_ns + rounded)
    };
    let date = adjust(vm, &dur.date, dur.date.days + delta, None, None)?;
    Ok(Nudge {
        dur: InternalDur { date, time: rounded },
        nudged_ns: nudged,
        expanded: beyond_day,
    })
}

/// NudgeToDayOrTime.
fn nudge_day_or_time(
    vm: &mut Vm,
    dur: &InternalDur,
    dest: i128,
    largest: Unit,
    inc: i64,
    smallest: Unit,
    mode: RoundingMode,
) -> Result<Nudge, Value> {
    let time = check_time(vm, dur.time + dur.date.days as i128 * NS_PER_DAY)?;
    let rounded = check_time(vm, round_i128(time, smallest.ns() * inc as i128, mode))?;
    let diff = rounded - time;
    let whole = time / NS_PER_DAY;
    let rounded_whole = rounded / NS_PER_DAY;
    let expanded = (rounded_whole - whole).signum() == time.signum();
    let nudged = dest + diff;
    let (days, rem) = if largest.is_date() {
        (rounded_whole, rounded - rounded_whole * NS_PER_DAY)
    } else {
        (0, rounded)
    };
    let date = adjust(vm, &dur.date, days as i64, None, None)?;
    Ok(Nudge {
        dur: InternalDur { date, time: rem },
        nudged_ns: nudged,
        expanded,
    })
}

/// BubbleRelativeDuration.
#[allow(clippy::too_many_arguments)]
fn bubble(
    vm: &mut Vm,
    sign: i64,
    mut dur: InternalDur,
    nudged: i128,
    dt: &DateTime,
    tz: Option<&Tz>,
    cal: Cal,
    largest: Unit,
    smallest: Unit,
) -> Result<InternalDur, Value> {
    if smallest == largest {
        return Ok(dur);
    }
    let (li, si) = (largest as usize, smallest as usize);
    for idx in (li..si).rev() {
        let unit = UNITS[idx];
        if unit == Unit::Week && largest != Unit::Week {
            continue;
        }
        let end = match unit {
            Unit::Year => DateDur {
                years: dur.date.years + sign,
                ..DateDur::default()
            },
            Unit::Month => adjust(vm, &dur.date, 0, Some(0), Some(dur.date.months + sign))?,
            _ => adjust(vm, &dur.date, 0, Some(dur.date.weeks + sign), None)?,
        };
        let e = calendar::date_add(vm, cal, &dt.date, &end, true)?;
        let end_ns = to_epoch(vm, tz, &e.at(dt.time))?;
        if iso::ord(nudged.cmp(&end_ns)) as i64 != -sign {
            dur = InternalDur { date: end, time: 0 };
        } else {
            break;
        }
    }
    Ok(dur)
}

/// RoundRelativeDuration.
#[allow(clippy::too_many_arguments)]
pub fn round_relative(
    vm: &mut Vm,
    dur: &InternalDur,
    origin: i128,
    dest: i128,
    dt: &DateTime,
    tz: Option<&Tz>,
    cal: Cal,
    largest: Unit,
    inc: i64,
    smallest: Unit,
    mode: RoundingMode,
) -> Result<InternalDur, Value> {
    let irregular = smallest.is_calendar() || (tz.is_some() && smallest == Unit::Day);
    let sign = if dur.sign() < 0 { -1 } else { 1 };
    let n = if irregular {
        nudge_calendar(vm, sign, dur, origin, dest, dt, tz, cal, inc, smallest, mode)?.0
    } else if let Some(tz) = tz {
        nudge_zoned(vm, sign, dur, dt, tz, cal, inc, smallest, mode)?
    } else {
        nudge_day_or_time(vm, dur, dest, largest, inc, smallest, mode)?
    };
    if n.expanded && smallest != Unit::Week {
        return bubble(vm, sign, n.dur, n.nudged_ns, dt, tz, cal, largest, larger(smallest, Unit::Day));
    }
    Ok(n.dur)
}

/// TotalRelativeDuration.
#[allow(clippy::too_many_arguments)]
pub fn total_relative(
    vm: &mut Vm,
    dur: &InternalDur,
    origin: i128,
    dest: i128,
    dt: &DateTime,
    tz: Option<&Tz>,
    cal: Cal,
    unit: Unit,
) -> Result<f64, Value> {
    if unit.is_calendar() || (tz.is_some() && unit == Unit::Day) {
        let sign = if dur.sign() < 0 { -1 } else { 1 };
        let (_, total) = nudge_calendar(vm, sign, dur, origin, dest, dt, tz, cal, 1, unit, RoundingMode::Trunc)?;
        return Ok(total);
    }
    let t = check_time(vm, dur.time + dur.date.days as i128 * NS_PER_DAY)?;
    Ok(fdiv(t, unit.ns()))
}

/// DifferencePlainDateTimeWithRounding.
#[allow(clippy::too_many_arguments)]
pub fn diff_date_time_rounded(
    vm: &mut Vm,
    a: &DateTime,
    b: &DateTime,
    cal: Cal,
    largest: Unit,
    inc: i64,
    smallest: Unit,
    mode: RoundingMode,
) -> Result<InternalDur, Value> {
    if a == b {
        return Ok(InternalDur::default());
    }
    if !iso::date_time_within_limits(a) || !iso::date_time_within_limits(b) {
        return range(vm, "Date and time are outside of supported range");
    }
    let d = diff_date_time(vm, a, b, cal, largest)?;
    if smallest == Unit::Nanosecond && inc == 1 {
        return Ok(d);
    }
    round_relative(vm, &d, iso::epoch_ns(a), iso::epoch_ns(b), a, None, cal, largest, inc, smallest, mode)
}

/// DifferencePlainDateTimeWithTotal.
pub fn diff_date_time_total(vm: &mut Vm, a: &DateTime, b: &DateTime, cal: Cal, unit: Unit) -> Result<f64, Value> {
    if a == b {
        return Ok(0.0);
    }
    if !iso::date_time_within_limits(a) || !iso::date_time_within_limits(b) {
        return range(vm, "Date and time are outside of supported range");
    }
    let d = diff_date_time(vm, a, b, cal, unit)?;
    if unit == Unit::Nanosecond {
        return Ok(d.time as f64);
    }
    total_relative(vm, &d, iso::epoch_ns(a), iso::epoch_ns(b), a, None, cal, unit)
}

/// DifferenceEpochNanoseconds.
pub fn diff_epoch(vm: &mut Vm, a: i128, b: i128, inc: i64, smallest: Unit, mode: RoundingMode) -> Result<InternalDur, Value> {
    Ok(InternalDur {
        date: DateDur::default(),
        time: round_time_dur(vm, b - a, inc, smallest, mode)?,
    })
}

/// DifferenceZonedDateTimeWithRounding.
#[allow(clippy::too_many_arguments)]
pub fn diff_zoned_rounded(
    vm: &mut Vm,
    a: i128,
    b: i128,
    tz: &Tz,
    cal: Cal,
    largest: Unit,
    inc: i64,
    smallest: Unit,
    mode: RoundingMode,
) -> Result<InternalDur, Value> {
    if !largest.is_date() {
        return diff_epoch(vm, a, b, inc, smallest, mode);
    }
    let d = diff_zoned(vm, a, b, tz, cal, largest)?;
    if smallest == Unit::Nanosecond && inc == 1 {
        return Ok(d);
    }
    let dt = zone::date_time_for(vm, tz, a);
    round_relative(vm, &d, a, b, &dt, Some(tz), cal, largest, inc, smallest, mode)
}

/// DifferenceZonedDateTimeWithTotal.
pub fn diff_zoned_total(vm: &mut Vm, a: i128, b: i128, tz: &Tz, cal: Cal, unit: Unit) -> Result<f64, Value> {
    if !unit.is_date() {
        return Ok(fdiv(b - a, unit.ns()));
    }
    let d = diff_zoned(vm, a, b, tz, cal, unit)?;
    let dt = zone::date_time_for(vm, tz, a);
    total_relative(vm, &d, a, b, &dt, Some(tz), cal, unit)
}

/// AddZonedDateTime.
pub fn add_zoned(vm: &mut Vm, ns: i128, tz: &Tz, cal: Cal, d: &InternalDur, constrain: bool) -> Result<i128, Value> {
    if d.date.sign() == 0 {
        return add_instant(vm, ns, d.time);
    }
    let dt = zone::date_time_for(vm, tz, ns);
    let added = calendar::date_add(vm, cal, &dt.date, &d.date, constrain)?;
    let mid = zone::epoch_for(vm, tz, &added.at(dt.time), Disambig::Compatible)?;
    add_instant(vm, mid, d.time)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arredonda() {
        assert_eq!(round_i128(15, 10, RoundingMode::HalfEven), 20);
        assert_eq!(round_i128(25, 10, RoundingMode::HalfEven), 20);
        assert_eq!(round_i128(-15, 10, RoundingMode::HalfExpand), -20);
        assert_eq!(round_i128(-15, 10, RoundingMode::Floor), -20);
        assert_eq!(round_i128(-15, 10, RoundingMode::Ceil), -10);
        assert_eq!(round_positive(-15, 10, RoundingMode::Trunc), -20);
        assert_eq!(round_positive(-15, 10, RoundingMode::HalfExpand), -10);
        assert_eq!(fdiv(1, 3), 1.0 / 3.0);
        assert_eq!(fdiv(-7, 2), -3.5);
    }
}
