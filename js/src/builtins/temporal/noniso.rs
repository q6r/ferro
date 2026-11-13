//! Algoritmos dos calendários não ISO sobre registros de campos (como na
//! implementação de referência): resolução de mês e era, conversão para
//! ISO com overflow, mês-dia de referência, soma e diferença de datas.

use super::calendar::{Cal, DateDur, DateField, Fields, FieldsKind, month_code_parts};
use super::cals::{self, CDate, MonthCode};
use super::iso::{self, IsoDate};
use super::round::Unit;
use super::{range, sat, type_err};
use crate::value::Value;
use crate::vm::Vm;

/// Data do calendário com o código do mês (CalendarISOToDate).
#[derive(Clone, Copy, Debug)]
pub struct CalDate {
    pub year: i64,
    pub month: u8,
    pub code: MonthCode,
    pub day: u8,
}

pub fn cal_date(cal: Cal, d: &IsoDate) -> CalDate {
    let c = cals::from_days(cal, iso::date_epoch_days(d));
    CalDate {
        year: c.year,
        month: c.month,
        code: cals::month_code(cal, c.year, c.month),
        day: c.day,
    }
}

/// Ano limitado (os absurdos são recusados depois, sem estourar a conta).
fn year_of(x: f64) -> i64 {
    sat(x).clamp(-1_000_000_000, 1_000_000_000)
}

fn parse_code(s: &str) -> MonthCode {
    let (num, leap) = month_code_parts(s).unwrap_or((0, true));
    MonthCode { num, leap }
}

/// Os campos com o mês resolvido, ainda sem limitar o dia.
struct Adjusted {
    year: i64,
    month: f64,
    day: f64,
}

/// resolveFields dos calendários não ISO (só valida).
pub fn resolve_fields(vm: &mut Vm, cal: Cal, f: &Fields, kind: FieldsKind) -> Result<(), Value> {
    let needs_year = |vm: &mut Vm| -> Result<(), Value> {
        if !cal.has_eras() {
            return type_err(vm, "Required property year is missing or undefined");
        }
        if f.era.is_none() || f.era_year.is_none() {
            return type_err(vm, "Required property year (or era and eraYear) is missing or undefined");
        }
        Ok(())
    };
    if kind != FieldsKind::MonthDay && f.year.is_none() {
        needs_year(vm)?;
    }
    if kind != FieldsKind::YearMonth && f.day.is_none() {
        return type_err(vm, "Required property day is missing or undefined");
    }
    if kind == FieldsKind::MonthDay && f.month.is_some() && f.year.is_none() {
        needs_year(vm)?;
    }
    if !cal.is_lunisolar() {
        non_lunisolar_month(vm, cal, f, None)?;
    }
    Ok(())
}

/// validateCalendarDate.
fn validate(vm: &mut Vm, cal: Cal, f: &Fields) -> Result<(), Value> {
    if f.year.is_none() && f.era_year.is_none() {
        return type_err(vm, "Required property year or eraYear is missing or undefined");
    }
    if f.month.is_none() && f.month_code.is_none() {
        return type_err(vm, "Required property month or monthCode is missing or undefined");
    }
    if f.day.is_none() {
        return range(vm, "Missing day");
    }
    if let Some(mc) = &f.month_code {
        let c = parse_code(mc);
        if !(1..=13).contains(&c.num) {
            return range(vm, format!("Invalid monthCode: {mc}"));
        }
    }
    if cal.has_eras() && f.era.is_none() != f.era_year.is_none() {
        return type_err(vm, "Properties era and eraYear must be provided together");
    }
    Ok(())
}

/// resolveNonLunisolarMonth: (mês, código). `constrain` None: sem overflow.
fn non_lunisolar_month(vm: &mut Vm, cal: Cal, f: &Fields, constrain: Option<bool>) -> Result<(f64, MonthCode), Value> {
    match &f.month_code {
        None => {
            let Some(mut month) = f.month else {
                return type_err(vm, "Required property month or monthCode is missing or undefined");
            };
            let mpy = cals::months_per_year(cal) as f64;
            match constrain {
                Some(false) if month > mpy => return range(vm, format!("Invalid month: {month}")),
                Some(true) => month = month.min(mpy),
                _ => {}
            }
            Ok((month, MonthCode::plain(month.min(99.0) as u8)))
        }
        Some(s) => {
            let mc = parse_code(s);
            if !cals::valid_month_code(cal, mc) {
                return range(vm, format!("Invalid monthCode: {s} does not exist in calendar {}", cal.id()));
            }
            if let Some(m) = f.month
                && m != mc.num as f64
            {
                return range(vm, format!("monthCode {s} and month {m} must match if both are present"));
            }
            Ok((mc.num as f64, mc))
        }
    }
}

/// completeEraYear: o ano aritmético (confere era e ano da era).
fn complete_era_year(vm: &mut Vm, cal: Cal, f: &Fields, month: f64, day: f64) -> Result<i64, Value> {
    let era_at = |y: i64| {
        let list = cals::eras(cal);
        let m = month.clamp(0.0, 255.0) as u8;
        let d = day.clamp(0.0, 255.0) as u8;
        for (i, e) in list.iter().enumerate() {
            if i == list.len() - 1 || (y, m, d) >= e.anchor {
                let ey = if i == list.len() - 1 && e.reverse { e.anchor.0 - y } else { y - e.anchor.0 + e.start };
                return (e.code, ey);
            }
        }
        ("", y)
    };
    if !cal.has_eras() {
        return Ok(year_of(f.year.unwrap_or(0.0)));
    }
    if let Some(y) = f.year {
        let y = year_of(y);
        let (era, ey) = era_at(y);
        if let Some(e) = &f.era
            && cals::canonical_era(cal, e).map(|x| x.code) != Some(era)
        {
            return range(vm, format!("Input era {e} doesn't match calculated value {era}"));
        }
        if let Some(v) = f.era_year
            && year_of(v) != ey
        {
            return range(vm, format!("Input eraYear {v} doesn't match calculated value {ey}"));
        }
        return Ok(y);
    }
    let era = f.era.clone().unwrap_or_default();
    let Some(e) = cals::canonical_era(cal, &era) else {
        return range(vm, format!("Invalid era {era} for calendar {}", cal.id()));
    };
    Ok(cals::year_of_era(e, year_of(f.era_year.unwrap_or(0.0))))
}

/// adjustCalendarDate: resolve o mês (com o overflow) e o ano.
fn adjust(vm: &mut Vm, cal: Cal, f: &Fields, constrain: bool) -> Result<Adjusted, Value> {
    let day = f.day.unwrap_or(1.0);
    match cal {
        Cal::Hebrew => {
            let year = complete_era_year(vm, cal, f, f.month.unwrap_or(0.0), day)?;
            validate(vm, cal, f)?;
            let leap = cals::in_leap_year(cal, year);
            let miy = cals::months_in_year(cal, year) as f64;
            let mut day = day;
            let month = match (f.month, &f.month_code) {
                (None, Some(s)) => {
                    let mc = parse_code(s);
                    if mc.leap {
                        if mc.num != 5 {
                            return range(vm, format!("Hebrew leap month must have monthCode M05L, not {s}"));
                        }
                        if !leap && !constrain {
                            return range(vm, format!("Hebrew monthCode M05L is invalid in year {year} which is not a leap year"));
                        }
                        6.0
                    } else {
                        let mut m = mc.num as f64;
                        if leap && m >= 6.0 {
                            m += 1.0;
                        }
                        if m < 1.0 || m > miy {
                            return range(vm, format!("Invalid monthCode: {s}"));
                        }
                        m
                    }
                }
                (Some(m), code) => {
                    let mut m = m;
                    if constrain {
                        m = m.clamp(1.0, miy);
                    } else if m > miy {
                        return range(vm, format!("Invalid month: {m}"));
                    }
                    let mc = cals::month_code(cal, year, m as u8);
                    let max = cals::max_day_of_code(cal, mc) as f64;
                    if constrain {
                        day = day.min(max);
                    } else if day > max {
                        return range(vm, format!("Invalid day: {day}"));
                    }
                    if let Some(s) = code
                        && parse_code(s) != mc
                    {
                        return range(vm, format!("monthCode {s} doesn't correspond to month {m} in Hebrew year {year}"));
                    }
                    m
                }
                (None, None) => 1.0,
            };
            Ok(Adjusted { year, month, day })
        }
        Cal::Chinese | Cal::Dangi => {
            validate(vm, cal, f)?;
            let year = year_of(f.year.unwrap_or(0.0));
            let mut day = day;
            let miy = cals::months_in_year(cal, year);
            let month = match (f.month, &f.month_code) {
                (None, Some(s)) => {
                    let mc = parse_code(s);
                    match cals::month_of_code(cal, year, mc) {
                        Some(m) => m as f64,
                        None => {
                            let plain = if mc.leap && constrain { cals::month_of_code(cal, year, MonthCode::plain(mc.num)) } else { None };
                            match plain {
                                Some(m) => m as f64,
                                None => return range(vm, format!("Unmatched month {s} in {} year {year}", cal.id())),
                            }
                        }
                    }
                }
                (Some(m), None) => {
                    if constrain {
                        day = day.min(30.0);
                        m.clamp(1.0, miy as f64)
                    } else {
                        if m > miy as f64 || day > 30.0 {
                            return range(vm, format!("Invalid month {m} or day {day}"));
                        }
                        m
                    }
                }
                (Some(m), Some(s)) => {
                    let mc = parse_code(s);
                    if cals::month_of_code(cal, year, mc).map(|x| x as f64) != Some(m) {
                        return range(vm, format!("monthCode {s} doesn't correspond to month {m} in {} year {year}", cal.id()));
                    }
                    m
                }
                (None, None) => 1.0,
            };
            Ok(Adjusted { year, month, day })
        }
        _ => {
            validate(vm, cal, f)?;
            let (month, _) = non_lunisolar_month(vm, cal, f, Some(constrain))?;
            let year = complete_era_year(vm, cal, f, month, day)?;
            Ok(Adjusted { year, month, day })
        }
    }
}

fn check_year(vm: &mut Vm, cal: Cal, year: i64) -> Result<(), Value> {
    let (lo, hi) = cals::year_range(cal);
    if year < lo || year > hi {
        return range(vm, format!("Year {year} out of range for calendar {}", cal.id()));
    }
    Ok(())
}

/// calendarToIsoDate: campos → data ISO (com overflow).
pub fn to_iso(vm: &mut Vm, cal: Cal, f: &Fields, constrain: bool) -> Result<IsoDate, Value> {
    let a = adjust(vm, cal, f, constrain)?;
    check_year(vm, cal, a.year)?;
    let miy = cals::months_in_year(cal, a.year) as f64;
    let month = if constrain {
        a.month.clamp(1.0, miy)
    } else if a.month < 1.0 || a.month > miy {
        return range(vm, format!("Invalid month: {}", a.month));
    } else {
        a.month
    } as u8;
    let dim = cals::days_in_month(cal, a.year, month) as f64;
    let day = if constrain {
        a.day.clamp(1.0, dim)
    } else if a.day < 1.0 || a.day > dim {
        return range(vm, format!("Invalid day {} for month {month} of year {}", a.day, a.year));
    } else {
        a.day
    } as u8;
    Ok(iso::from_epoch_days(cals::to_days(cal, a.year, month, day)))
}

/// monthDayFromFields: data ISO de referência de um mês-dia.
pub fn month_day(vm: &mut Vm, cal: Cal, f: &Fields, constrain: bool) -> Result<IsoDate, Value> {
    let (mut mc, mut day) = if f.month_code.is_none() || f.year.is_some() || (cal.has_eras() && f.era_year.is_some()) {
        let d = to_iso(vm, cal, f, constrain)?;
        let c = cal_date(cal, &d);
        (c.code, c.day as f64)
    } else {
        (parse_code(f.month_code.as_deref().unwrap_or("")), f.day.unwrap_or(1.0))
    };
    if !cals::valid_month_code(cal, mc) {
        return range(vm, format!("Invalid monthCode: {} does not exist in calendar {}", mc.string(), cal.id()));
    }
    if matches!(cal, Cal::Chinese | Cal::Dangi) {
        let d = (day.min(30.0)) as u8;
        if cals::reference_year(cal, mc, 1).is_none() || (d >= 30 && cals::reference_year(cal, mc, 30).is_none()) {
            if !constrain {
                return range(vm, format!("No {} year known to exist with month code {} and day {day}", cal.id(), mc.string()));
            }
            mc = MonthCode::plain(mc.num);
        }
    }
    let max = cals::max_day_of_code(cal, mc) as f64;
    if day > max {
        if !constrain {
            return range(vm, format!("No {} year with month code {} and day {day}", cal.id(), mc.string()));
        }
        day = max;
    }
    let Some(ref_year) = cals::reference_year(cal, mc, day as u8) else {
        return range(vm, format!("No {} year with month code {}", cal.id(), mc.string()));
    };
    let start = cal_date(cal, &IsoDate::new(ref_year, 12, 31));
    let year = if start.code > mc || (start.code == mc && start.day as f64 >= day) { start.year } else { start.year - 1 };
    for i in 0..2 {
        let test = Fields {
            year: Some((year - i) as f64),
            month_code: Some(mc.string()),
            day: Some(day),
            ..Default::default()
        };
        let d = to_iso(vm, cal, &test, true)?;
        let c = cal_date(cal, &d);
        if c.code == mc && c.day as f64 == day {
            return Ok(d);
        }
    }
    range(vm, format!("No reference year for {} day {day} in calendar {}", mc.string(), cal.id()))
}

/// Avança `n` meses a partir de (ano, mês). None: fora do intervalo.
fn step_months(cal: Cal, mut y: i64, mut m: u8, n: i64) -> Option<(i64, u8)> {
    if !cal.is_lunisolar() {
        let mpy = cals::months_per_year(cal) as i64;
        let t = m as i64 - 1 + n;
        return Some((y + t.div_euclid(mpy), (t.rem_euclid(mpy) + 1) as u8));
    }
    let (lo, hi) = cals::year_range(cal);
    for _ in 0..n.unsigned_abs() {
        if n > 0 {
            if m < cals::months_in_year(cal, y) {
                m += 1;
            } else {
                y += 1;
                m = 1;
            }
        } else if m > 1 {
            m -= 1;
        } else {
            y -= 1;
            m = cals::months_in_year(cal, y);
        }
        if y < lo - 1 || y > hi + 1 {
            return None;
        }
    }
    Some((y, m))
}

/// addCalendar sem os dias: (ano, mês, dia limitado ao mês).
#[allow(clippy::too_many_arguments)]
fn add_ym(vm: &mut Vm, cal: Cal, year: i64, code: MonthCode, day: u8, years: i64, months: i64, constrain: bool) -> Result<CDate, Value> {
    let (mut years, mut months) = (years, months);
    if let Some((cy, cm)) = cals::cycle(cal)
        && months.abs() > cm
    {
        years += (months / cm) * cy;
        months %= cm;
    }
    let f = Fields {
        year: Some(year.saturating_add(years) as f64),
        month_code: Some(code.string()),
        day: Some(day as f64),
        ..Default::default()
    };
    let a = adjust(vm, cal, &f, constrain)?;
    check_year(vm, cal, a.year)?;
    let Some((y, m)) = step_months(cal, a.year, a.month as u8, months) else {
        return range(vm, "Date is outside of supported range");
    };
    check_year(vm, cal, y)?;
    let dim = cals::days_in_month(cal, y, m);
    let mut d = a.day as u8;
    if d > dim {
        if !constrain {
            return range(vm, format!("Day {d} does not exist in resulting calendar month"));
        }
        d = dim;
    }
    Ok(CDate { year: y, month: m, day: d })
}

/// CalendarDateAdd (não ISO).
pub fn date_add(vm: &mut Vm, cal: Cal, d: &IsoDate, dur: &DateDur, constrain: bool) -> Result<IsoDate, Value> {
    let c = cal_date(cal, d);
    let r = add_ym(vm, cal, c.year, c.code, c.day, dur.years, dur.months, constrain)?;
    let days = dur.days.saturating_add(dur.weeks.saturating_mul(7));
    let z = cals::to_days(cal, r.year, r.month, r.day).saturating_add(days);
    let out = iso::from_epoch_days(z);
    if !iso::date_within_limits(&out) {
        return range(vm, "Date is outside of supported range");
    }
    Ok(out)
}

fn surpasses(sign: i64, y: i64, m: u8, d: u8, t: &CalDate) -> bool {
    if y != t.year {
        sign * (y - t.year) > 0
    } else if m != t.month {
        sign * (m as i64 - t.month as i64) > 0
    } else if d != t.day {
        sign * (d as i64 - t.day as i64) > 0
    } else {
        false
    }
}

/// CalendarDateUntil (não ISO).
pub fn date_until(vm: &mut Vm, cal: Cal, one: &IsoDate, two: &IsoDate, largest: Unit) -> DateDur {
    if largest == Unit::Week || largest == Unit::Day {
        let mut days = iso::date_epoch_days(two) - iso::date_epoch_days(one);
        let mut weeks = 0;
        if largest == Unit::Week {
            weeks = days / 7;
            days %= 7;
        }
        return DateDur { years: 0, months: 0, weeks, days };
    }
    let a = cal_date(cal, one);
    let b = cal_date(cal, two);
    let sign = iso::ord((b.year, b.month, b.day).cmp(&(a.year, a.month, a.day))) as i64;
    if sign == 0 {
        return DateDur::default();
    }
    let diff_years = b.year - a.year;
    let diff_days = b.day as i64 - a.day as i64;
    let cyc = cals::cycle(cal);
    let (mut years, mut months) = (0i64, 0i64);
    if diff_years != 0 && (largest == Unit::Year || cyc.is_some()) {
        let in_year = match b.code.cmp(&a.code) {
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => diff_days.signum(),
        };
        years = if in_year * sign < 0 { diff_years - sign } else { diff_years };
    }
    if largest == Unit::Month {
        if let Some((cy, cm)) = cyc
            && years.abs() >= cy
        {
            months = (years / cy) * cm;
        }
        years = 0;
    }
    let start = CDate {
        year: a.year,
        month: a.month,
        day: a.day,
    };
    let add = |vm: &mut Vm, years: i64, months: i64| -> CDate {
        if years == 0 && months == 0 {
            return start;
        }
        add_ym(vm, cal, a.year, a.code, a.day, years, months, true).unwrap_or(start)
    };
    let mid = add(vm, years, months);
    if surpasses(sign, mid.year, mid.month, a.day, &b) {
        years -= sign;
    }
    let mut next = add(vm, years, months);
    next.day = a.day;
    let mut current;
    loop {
        months += sign;
        current = next;
        match step_months(cal, current.year, current.month, sign) {
            Some((y, m)) => next = CDate { year: y, month: m, day: a.day },
            None => break,
        }
        if surpasses(sign, next.year, next.month, next.day, &b) {
            break;
        }
    }
    months -= sign;
    let cd = current.day.min(cals::days_in_month(cal, current.year, current.month));
    let days = iso::date_epoch_days(two) - cals::to_days(cal, current.year, current.month, cd);
    DateDur { years, months, weeks: 0, days }
}

/// CalendarISOToDate para os getters.
pub fn date_field(d: &IsoDate, cal: Cal, f: DateField) -> Value {
    let c = cal_date(cal, d);
    let n = |x: i64| Value::Number(x as f64);
    let era = || cals::era_of(cal, c.year, c.month, c.day);
    match f {
        DateField::Era => era().map_or(Value::Undefined, |(e, _)| Value::str(e)),
        DateField::EraYear => era().map_or(Value::Undefined, |(_, y)| n(y)),
        DateField::Year => n(c.year),
        DateField::Month => n(c.month as i64),
        DateField::MonthCode => Value::str(&c.code.string()),
        DateField::Day => n(c.day as i64),
        DateField::DayOfWeek => n(iso::day_of_week(d) as i64),
        DateField::DayOfYear => n(iso::date_epoch_days(d) - cals::to_days(cal, c.year, 1, 1) + 1),
        DateField::WeekOfYear | DateField::YearOfWeek => Value::Undefined,
        DateField::DaysInWeek => n(7),
        DateField::DaysInMonth => n(cals::days_in_month(cal, c.year, c.month) as i64),
        DateField::DaysInYear => n(cals::days_in_year(cal, c.year)),
        DateField::MonthsInYear => n(cals::months_in_year(cal, c.year) as i64),
        DateField::InLeapYear => Value::Bool(cals::in_leap_year(cal, c.year)),
    }
}
