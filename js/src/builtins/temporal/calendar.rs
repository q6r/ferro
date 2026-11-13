//! Calendários: identificadores, registros de campos (PrepareCalendarFields,
//! CalendarMergeFields) e a aritmética de datas. O ISO 8601 é calculado aqui;
//! os outros (gregoriano, japonês, hebraico, chinês...) em `noniso`/`cals`.

use super::iso::{self, IsoDate};
use super::noniso;
use super::parse;
use super::zone::{self, Tz};
use super::{TemporalData, range, sat, to_int_trunc, to_positive_int, type_err};
use crate::value::*;
use crate::vm::{Hint, Vm};

/// Calendário de um objeto Temporal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cal {
    Iso,
    Buddhist,
    Chinese,
    Coptic,
    Dangi,
    Ethioaa,
    Ethiopic,
    Gregory,
    Hebrew,
    Indian,
    IslamicCivil,
    IslamicTbla,
    IslamicUmalqura,
    Japanese,
    Persian,
    Roc,
}

impl Cal {
    pub fn id(self) -> &'static str {
        match self {
            Cal::Iso => "iso8601",
            Cal::Buddhist => "buddhist",
            Cal::Chinese => "chinese",
            Cal::Coptic => "coptic",
            Cal::Dangi => "dangi",
            Cal::Ethioaa => "ethioaa",
            Cal::Ethiopic => "ethiopic",
            Cal::Gregory => "gregory",
            Cal::Hebrew => "hebrew",
            Cal::Indian => "indian",
            Cal::IslamicCivil => "islamic-civil",
            Cal::IslamicTbla => "islamic-tbla",
            Cal::IslamicUmalqura => "islamic-umalqura",
            Cal::Japanese => "japanese",
            Cal::Persian => "persian",
            Cal::Roc => "roc",
        }
    }

    /// Calendário com eras (campos era e eraYear).
    pub fn has_eras(self) -> bool {
        !matches!(self, Cal::Iso | Cal::Chinese | Cal::Dangi)
    }

    /// Lunissolar: meses intercalares, número de meses varia.
    pub fn is_lunisolar(self) -> bool {
        matches!(self, Cal::Hebrew | Cal::Chinese | Cal::Dangi)
    }
}

/// CanonicalizeCalendar (None: calendário desconhecido).
pub fn canonicalize(id: &str) -> Option<Cal> {
    Some(match id.to_ascii_lowercase().as_str() {
        "iso8601" => Cal::Iso,
        "buddhist" => Cal::Buddhist,
        "chinese" => Cal::Chinese,
        "coptic" => Cal::Coptic,
        "dangi" => Cal::Dangi,
        "ethioaa" | "ethiopic-amete-alem" => Cal::Ethioaa,
        "ethiopic" => Cal::Ethiopic,
        "gregory" => Cal::Gregory,
        "hebrew" => Cal::Hebrew,
        "indian" => Cal::Indian,
        "islamic-civil" | "islamicc" => Cal::IslamicCivil,
        "islamic-tbla" => Cal::IslamicTbla,
        "islamic-umalqura" => Cal::IslamicUmalqura,
        "japanese" => Cal::Japanese,
        "persian" => Cal::Persian,
        "roc" => Cal::Roc,
        _ => return None,
    })
}

pub fn canonicalize_or_throw(vm: &mut Vm, id: &str) -> Result<Cal, Value> {
    match canonicalize(id) {
        Some(c) => Ok(c),
        None => range(vm, format!("Invalid calendar: {id}")),
    }
}

/// Calendário guardado num objeto Temporal (com [[Calendar]]).
pub fn slot_calendar(v: &Value) -> Option<Cal> {
    match super::data(v)? {
        TemporalData::Date(_, c)
        | TemporalData::DateTime(_, c)
        | TemporalData::YearMonth(_, c)
        | TemporalData::MonthDay(_, c)
        | TemporalData::Zoned(_, _, c) => Some(c),
        _ => None,
    }
}

/// ToTemporalCalendarIdentifier.
pub fn to_calendar(vm: &mut Vm, v: &Value) -> Result<Cal, Value> {
    if let Some(c) = slot_calendar(v) {
        return Ok(c);
    }
    let Value::String(s) = v else {
        let d = vm.describe(v);
        return type_err(vm, format!("Invalid calendar: {d} is not a string"));
    };
    let s = s.to_rust();
    // ParseTemporalCalendarString
    let id = match parse::parse_iso(&s, &parse::ALL_GOALS) {
        Ok(p) => p.calendar.unwrap_or_else(|| "iso8601".into()),
        Err(_) => {
            if !parse::is_annotation_value(&s) {
                return range(vm, format!("Invalid calendar: {s}"));
            }
            s
        }
    };
    canonicalize_or_throw(vm, &id)
}

/// GetTemporalCalendarIdentifierWithISODefault.
pub fn calendar_with_iso_default(vm: &mut Vm, item: &Value) -> Result<Cal, Value> {
    if let Some(c) = slot_calendar(item) {
        return Ok(c);
    }
    let c = vm.get_str(item, "calendar")?;
    if c.is_undefined() {
        return Ok(Cal::Iso);
    }
    to_calendar(vm, &c)
}

/// FormatCalendarAnnotation.
pub fn annotation(cal: Cal, show: ShowCalendar) -> String {
    match show {
        ShowCalendar::Never => String::new(),
        ShowCalendar::Auto if cal == Cal::Iso => String::new(),
        ShowCalendar::Critical => format!("[!u-ca={}]", cal.id()),
        _ => format!("[u-ca={}]", cal.id()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShowCalendar {
    Auto,
    Always,
    Never,
    Critical,
}

// ------------------------------------------------- registro de campos

/// Calendar Fields Record (None = ~unset~).
#[derive(Clone, Debug, Default)]
pub struct Fields {
    pub era: Option<String>,
    pub era_year: Option<f64>,
    pub year: Option<f64>,
    pub month: Option<f64>,
    pub month_code: Option<String>,
    pub day: Option<f64>,
    pub hour: Option<f64>,
    pub minute: Option<f64>,
    pub second: Option<f64>,
    pub millisecond: Option<f64>,
    pub microsecond: Option<f64>,
    pub nanosecond: Option<f64>,
    pub offset: Option<String>,
    pub time_zone: Option<Tz>,
}

pub const F_DAY: u16 = 1;
pub const F_ERA: u16 = 2;
pub const F_ERA_YEAR: u16 = 4;
pub const F_HOUR: u16 = 8;
pub const F_MICROSECOND: u16 = 16;
pub const F_MILLISECOND: u16 = 32;
pub const F_MINUTE: u16 = 64;
pub const F_MONTH: u16 = 128;
pub const F_MONTH_CODE: u16 = 256;
pub const F_NANOSECOND: u16 = 512;
pub const F_OFFSET: u16 = 1024;
pub const F_SECOND: u16 = 2048;
pub const F_TIME_ZONE: u16 = 4096;
pub const F_YEAR: u16 = 8192;

pub const DATE_FIELDS: u16 = F_DAY | F_MONTH | F_MONTH_CODE | F_YEAR;
pub const YEAR_MONTH_FIELDS: u16 = F_MONTH | F_MONTH_CODE | F_YEAR;
pub const TIME_FIELDS: u16 = F_HOUR | F_MINUTE | F_SECOND | F_MILLISECOND | F_MICROSECOND | F_NANOSECOND;

/// Propriedades em ordem lexicográfica (a ordem das leituras).
const ORDER: [(&str, u16); 14] = [
    ("day", F_DAY),
    ("era", F_ERA),
    ("eraYear", F_ERA_YEAR),
    ("hour", F_HOUR),
    ("microsecond", F_MICROSECOND),
    ("millisecond", F_MILLISECOND),
    ("minute", F_MINUTE),
    ("month", F_MONTH),
    ("monthCode", F_MONTH_CODE),
    ("nanosecond", F_NANOSECOND),
    ("offset", F_OFFSET),
    ("second", F_SECOND),
    ("timeZone", F_TIME_ZONE),
    ("year", F_YEAR),
];

/// Campos obrigatórios em PrepareCalendarFields.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Required {
    /// Lista vazia: horas em falta valem 0.
    None,
    /// Só o fuso é obrigatório.
    TimeZone,
    /// ~partial~: pelo menos um campo.
    Partial,
}

/// ParseMonthCode → (número, mês intercalar).
pub fn parse_month_code(vm: &mut Vm, v: &Value) -> Result<(u8, bool), Value> {
    let p = vm.to_primitive(v, Hint::String)?;
    let Value::String(s) = p else {
        let d = vm.describe(&p);
        return type_err(vm, format!("Invalid monthCode: {d} is not a string"));
    };
    let s = s.to_rust();
    match month_code_parts(&s) {
        Some(r) => Ok(r),
        None => range(vm, format!("Invalid monthCode: {s}")),
    }
}

/// Gramática MonthCode: M00L, M0dL?, Mdd L?.
pub fn month_code_parts(s: &str) -> Option<(u8, bool)> {
    let b = s.as_bytes();
    if !(b.len() == 3 || b.len() == 4) || b[0] != b'M' || !b[1].is_ascii_digit() || !b[2].is_ascii_digit() {
        return None;
    }
    let leap = b.len() == 4;
    if leap && b[3] != b'L' {
        return None;
    }
    let n = (b[1] - b'0') * 10 + (b[2] - b'0');
    if n == 0 && !leap {
        return None;
    }
    Some((n, leap))
}

pub fn month_code(n: u8) -> String {
    format!("M{n:02}")
}

/// PrepareCalendarFields.
pub fn prepare(vm: &mut Vm, cal: Cal, bag: &Value, names: u16, required: Required) -> Result<Fields, Value> {
    // CalendarExtraFields: era e eraYear junto com year
    let names = if cal.has_eras() && names & F_YEAR != 0 { names | F_ERA | F_ERA_YEAR } else { names };
    let mut f = Fields::default();
    let mut any = false;
    for (name, flag) in ORDER {
        if names & flag == 0 {
            continue;
        }
        let v = vm.get_str(bag, name)?;
        if v.is_undefined() {
            if required == Required::TimeZone && flag == F_TIME_ZONE {
                return type_err(vm, "Required property timeZone is missing or undefined");
            }
            continue;
        }
        any = true;
        match flag {
            F_DAY => f.day = Some(to_positive_int(vm, &v, "day")?),
            F_ERA => f.era = Some(vm.to_rust_string(&v)?),
            F_ERA_YEAR => f.era_year = Some(to_int_trunc(vm, &v, "eraYear")?),
            F_HOUR => f.hour = Some(to_int_trunc(vm, &v, "hour")?),
            F_MICROSECOND => f.microsecond = Some(to_int_trunc(vm, &v, "microsecond")?),
            F_MILLISECOND => f.millisecond = Some(to_int_trunc(vm, &v, "millisecond")?),
            F_MINUTE => f.minute = Some(to_int_trunc(vm, &v, "minute")?),
            F_MONTH => f.month = Some(to_positive_int(vm, &v, "month")?),
            F_MONTH_CODE => {
                let (n, leap) = parse_month_code(vm, &v)?;
                f.month_code = Some(if leap { format!("M{n:02}L") } else { month_code(n) });
            }
            F_NANOSECOND => f.nanosecond = Some(to_int_trunc(vm, &v, "nanosecond")?),
            F_OFFSET => {
                // ToOffsetString
                let p = vm.to_primitive(&v, Hint::String)?;
                let Value::String(s) = p else {
                    let d = vm.describe(&p);
                    return type_err(vm, format!("Invalid offset: {d} is not a string"));
                };
                let s = s.to_rust();
                if parse::parse_utc_offset(&s).is_none() {
                    return range(vm, format!("Invalid offset string: {s}"));
                }
                f.offset = Some(s);
            }
            F_SECOND => f.second = Some(to_int_trunc(vm, &v, "second")?),
            F_TIME_ZONE => f.time_zone = Some(zone::to_time_zone(vm, &v)?),
            _ => f.year = Some(to_int_trunc(vm, &v, "year")?),
        }
    }
    if required == Required::Partial && !any {
        return type_err(vm, "Invalid partial object: no recognized properties");
    }
    if required != Required::Partial && names & TIME_FIELDS != 0 {
        for x in [&mut f.hour, &mut f.minute, &mut f.second, &mut f.millisecond, &mut f.microsecond, &mut f.nanosecond] {
            x.get_or_insert(0.0);
        }
    }
    Ok(f)
}

/// CalendarMergeFields (CalendarFieldKeysToIgnore: month e monthCode se
/// excluem; nos calendários com eras, year, era e eraYear também).
pub fn merge(cal: Cal, fields: &Fields, add: &Fields) -> Fields {
    let mut m = fields.clone();
    if cal != Cal::Iso {
        if add.era.is_some() || add.era_year.is_some() || add.year.is_some() {
            m.era = None;
            m.era_year = None;
            m.year = None;
        }
        // eras que começam no meio do ano: mudar mês ou dia muda a era
        if cal == Cal::Japanese && (add.month.is_some() || add.month_code.is_some() || add.day.is_some()) {
            m.era = None;
            m.era_year = None;
        }
    }
    macro_rules! take {
        ($($f:ident),*) => {$(
            if add.$f.is_some() {
                m.$f = add.$f.clone();
            }
        )*};
    }
    if add.month.is_some() || add.month_code.is_some() {
        m.month = None;
        m.month_code = None;
    }
    take!(era, era_year, year, month, month_code, day, hour, minute, second, millisecond, microsecond, nanosecond, offset, time_zone);
    m
}

/// ISODateToFields.
pub fn date_to_fields(cal: Cal, d: &IsoDate, kind: FieldsKind) -> Fields {
    let (year, code, day) = if cal == Cal::Iso {
        (d.year, month_code(d.month), d.day)
    } else {
        let c = noniso::cal_date(cal, d);
        (c.year, c.code.string(), c.day)
    };
    let mut f = Fields {
        month_code: Some(code),
        ..Default::default()
    };
    if kind != FieldsKind::YearMonth {
        f.day = Some(day as f64);
    }
    if kind != FieldsKind::MonthDay {
        f.year = Some(year as f64);
    }
    f
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FieldsKind {
    Date,
    YearMonth,
    MonthDay,
}

/// CalendarResolveFields (ISO).
fn resolve(vm: &mut Vm, f: &mut Fields, kind: FieldsKind) -> Result<(), Value> {
    if kind != FieldsKind::MonthDay && f.year.is_none() {
        return type_err(vm, "Required property year is missing or undefined");
    }
    if kind != FieldsKind::YearMonth && f.day.is_none() {
        return type_err(vm, "Required property day is missing or undefined");
    }
    if f.month.is_none() && f.month_code.is_none() {
        return type_err(vm, "Required property month or monthCode is missing or undefined");
    }
    if let Some(mc) = &f.month_code {
        let (n, leap) = month_code_parts(mc).unwrap_or((0, true));
        if leap || n > 12 {
            return range(vm, format!("Invalid monthCode for the ISO 8601 calendar: {mc}"));
        }
        if let Some(m) = f.month
            && m != n as f64
        {
            return range(vm, format!("monthCode {mc} and month {m} must match if both are present"));
        }
        f.month = Some(n as f64);
    }
    Ok(())
}

fn regulate(vm: &mut Vm, y: f64, m: f64, d: f64, constrain: bool) -> Result<IsoDate, Value> {
    match iso::regulate_date(sat(y), sat(m), sat(d), constrain) {
        Some(r) => Ok(r),
        None => range(vm, "Invalid date: value out of range"),
    }
}

/// CalendarDateFromFields.
pub fn date_from_fields(vm: &mut Vm, cal: Cal, mut f: Fields, constrain: bool) -> Result<IsoDate, Value> {
    let d = if cal == Cal::Iso {
        resolve(vm, &mut f, FieldsKind::Date)?;
        regulate(vm, f.year.unwrap_or(0.0), f.month.unwrap_or(1.0), f.day.unwrap_or(1.0), constrain)?
    } else {
        noniso::resolve_fields(vm, cal, &f, FieldsKind::Date)?;
        noniso::to_iso(vm, cal, &f, constrain)?
    };
    if !iso::date_within_limits(&d) {
        return range(vm, "Date is outside of supported range");
    }
    Ok(d)
}

/// CalendarYearMonthFromFields.
pub fn year_month_from_fields(vm: &mut Vm, cal: Cal, mut f: Fields, constrain: bool) -> Result<IsoDate, Value> {
    f.day = Some(1.0);
    let d = if cal == Cal::Iso {
        resolve(vm, &mut f, FieldsKind::YearMonth)?;
        regulate(vm, f.year.unwrap_or(0.0), f.month.unwrap_or(1.0), 1.0, constrain)?
    } else {
        noniso::resolve_fields(vm, cal, &f, FieldsKind::YearMonth)?;
        noniso::to_iso(vm, cal, &f, constrain)?
    };
    if !iso::year_month_within_limits(d.year, d.month) {
        return range(vm, "Year and month are outside of supported range");
    }
    Ok(d)
}

/// CalendarMonthDayFromFields.
pub fn month_day_from_fields(vm: &mut Vm, cal: Cal, mut f: Fields, constrain: bool) -> Result<IsoDate, Value> {
    if cal != Cal::Iso {
        noniso::resolve_fields(vm, cal, &f, FieldsKind::MonthDay)?;
        let d = noniso::month_day(vm, cal, &f, constrain)?;
        if !iso::date_within_limits(&d) {
            return range(vm, "Date is outside of supported range");
        }
        return Ok(d);
    }
    resolve(vm, &mut f, FieldsKind::MonthDay)?;
    let y = f.year.unwrap_or(1972.0);
    let d = regulate(vm, y, f.month.unwrap_or(1.0), f.day.unwrap_or(1.0), constrain)?;
    Ok(IsoDate::new(1972, d.month, d.day))
}

// ------------------------------------------------- aritmética de datas

/// Date Duration Record.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DateDur {
    pub years: i64,
    pub months: i64,
    pub weeks: i64,
    pub days: i64,
}

impl DateDur {
    pub fn sign(&self) -> i64 {
        for v in [self.years, self.months, self.weeks, self.days] {
            if v != 0 {
                return v.signum();
            }
        }
        0
    }
}

/// CalendarDateAdd.
pub fn date_add(vm: &mut Vm, cal: Cal, d: &IsoDate, dur: &DateDur, constrain: bool) -> Result<IsoDate, Value> {
    if cal != Cal::Iso {
        return noniso::date_add(vm, cal, d, dur, constrain);
    }
    let (y, m) = iso::balance_year_month(d.year.saturating_add(dur.years), d.month as i64 + dur.months);
    let Some(mid) = iso::regulate_date(y, m as i64, d.day as i64, constrain) else {
        return range(vm, "Invalid date: day out of range for the month");
    };
    let days = dur.days.saturating_add(dur.weeks.saturating_mul(7));
    let r = iso::add_days(mid, days);
    if !iso::date_within_limits(&r) {
        return range(vm, "Date is outside of supported range");
    }
    Ok(r)
}

fn surpasses_ymd(sign: i64, y: i64, m: i64, d: i64, t: &IsoDate) -> bool {
    if y != t.year {
        sign * (y - t.year) > 0
    } else if m != t.month as i64 {
        sign * (m - t.month as i64) > 0
    } else if d != t.day as i64 {
        sign * (d - t.day as i64) > 0
    } else {
        false
    }
}

/// ISODateSurpasses (anos e meses; o dia não é limitado ao mês).
fn surpasses(sign: i64, base: &IsoDate, years: i64, months: i64, t: &IsoDate) -> bool {
    let y0 = base.year + years;
    if surpasses_ymd(sign, y0, base.month as i64, base.day as i64, t) {
        return true;
    }
    if months == 0 {
        return false;
    }
    let (y, m) = iso::balance_year_month(y0, base.month as i64 + months);
    surpasses_ymd(sign, y, m as i64, base.day as i64, t)
}

/// CalendarDateUntil.
pub fn date_until(vm: &mut Vm, cal: Cal, one: &IsoDate, two: &IsoDate, largest: super::round::Unit) -> DateDur {
    use super::round::Unit;
    let sign = -(iso::cmp_date(one, two) as i64);
    if sign == 0 {
        return DateDur::default();
    }
    if cal != Cal::Iso {
        return noniso::date_until(vm, cal, one, two, largest);
    }
    let (mut years, mut months) = (0i64, 0i64);
    if largest == Unit::Year || largest == Unit::Month {
        let mut cand = two.year - one.year;
        if cand != 0 {
            cand -= sign;
        }
        while !surpasses(sign, one, cand, 0, two) {
            years = cand;
            cand += sign;
        }
        let mut cand = sign;
        while !surpasses(sign, one, years, cand, two) {
            months = cand;
            cand += sign;
        }
        if largest == Unit::Month {
            months += years * 12;
            years = 0;
        }
    }
    let (y, m) = iso::balance_year_month(one.year + years, one.month as i64 + months);
    let mid = iso::constrain_date(y, m as i64, one.day as i64);
    let mut days = iso::date_epoch_days(two) - iso::date_epoch_days(&mid);
    let mut weeks = 0;
    if largest == Unit::Week {
        weeks = days / 7;
        days %= 7;
    }
    DateDur { years, months, weeks, days }
}

/// InterpretTemporalDateTimeFields.
pub fn interpret_date_time(vm: &mut Vm, cal: Cal, f: &Fields, constrain: bool) -> Result<iso::DateTime, Value> {
    let date = date_from_fields(vm, cal, f.clone(), constrain)?;
    let g = |x: Option<f64>| sat(x.unwrap_or(0.0));
    match iso::regulate_time(g(f.hour), g(f.minute), g(f.second), g(f.millisecond), g(f.microsecond), g(f.nanosecond), constrain) {
        Some(t) => Ok(date.at(t)),
        None => range(vm, "Invalid time: value out of range"),
    }
}

/// Campo de calendário de uma data (getters year, month, dayOfWeek...).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DateField {
    Era,
    EraYear,
    Year,
    Month,
    MonthCode,
    Day,
    DayOfWeek,
    DayOfYear,
    WeekOfYear,
    YearOfWeek,
    DaysInWeek,
    DaysInMonth,
    DaysInYear,
    MonthsInYear,
    InLeapYear,
}

/// CalendarISOToDate, só o campo pedido.
pub fn date_field(d: &IsoDate, cal: Cal, f: DateField) -> Value {
    if cal != Cal::Iso {
        return noniso::date_field(d, cal, f);
    }
    let n = |x: i64| Value::Number(x as f64);
    match f {
        DateField::Era | DateField::EraYear => Value::Undefined,
        DateField::Year => n(d.year),
        DateField::Month => n(d.month as i64),
        DateField::MonthCode => Value::str(&month_code(d.month)),
        DateField::Day => n(d.day as i64),
        DateField::DayOfWeek => n(iso::day_of_week(d) as i64),
        DateField::DayOfYear => n(iso::day_of_year(d) as i64),
        DateField::WeekOfYear => n(iso::week_of_year(d).0 as i64),
        DateField::YearOfWeek => n(iso::week_of_year(d).1),
        DateField::DaysInWeek => n(7),
        DateField::DaysInMonth => n(iso::days_in_month(d.year, d.month) as i64),
        DateField::DaysInYear => n(iso::days_in_year(d.year) as i64),
        DateField::MonthsInYear => n(12),
        DateField::InLeapYear => Value::Bool(iso::is_leap(d.year)),
    }
}

/// Getters de calendário de cada classe: (nome, campo).
pub const DATE_GETTERS: [(&str, DateField); 15] = [
    ("era", DateField::Era),
    ("eraYear", DateField::EraYear),
    ("year", DateField::Year),
    ("month", DateField::Month),
    ("monthCode", DateField::MonthCode),
    ("day", DateField::Day),
    ("dayOfWeek", DateField::DayOfWeek),
    ("dayOfYear", DateField::DayOfYear),
    ("weekOfYear", DateField::WeekOfYear),
    ("yearOfWeek", DateField::YearOfWeek),
    ("daysInWeek", DateField::DaysInWeek),
    ("daysInMonth", DateField::DaysInMonth),
    ("daysInYear", DateField::DaysInYear),
    ("monthsInYear", DateField::MonthsInYear),
    ("inLeapYear", DateField::InLeapYear),
];
