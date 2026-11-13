//! Calendário ISO 8601: datas, horas, dias desde a época, balanceamento e
//! os limites do Temporal.

use std::cmp::Ordering;

pub const NS_PER_DAY: i128 = 86_400_000_000_000;
/// Limite dos instantes: 10^8 dias antes ou depois de 1970.
pub const NS_MAX: i128 = NS_PER_DAY * 100_000_000;
pub const NS_MIN: i128 = -NS_MAX;
/// Maior duração de tempo representável: 2^53 s - 1 ns.
pub const MAX_TIME_DURATION: i128 = 9_007_199_254_740_991_999_999_999;

/// Data ISO com mês e dia coerentes; o ano pode estar fora dos limites do
/// Temporal enquanto não for conferido.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct IsoDate {
    pub year: i64,
    pub month: u8,
    pub day: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Time {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub ms: u16,
    pub us: u16,
    pub ns: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct DateTime {
    pub date: IsoDate,
    pub time: Time,
}

pub const MIDNIGHT: Time = Time {
    hour: 0,
    minute: 0,
    second: 0,
    ms: 0,
    us: 0,
    ns: 0,
};

pub const NOON: Time = Time {
    hour: 12,
    minute: 0,
    second: 0,
    ms: 0,
    us: 0,
    ns: 0,
};

impl IsoDate {
    pub fn new(year: i64, month: u8, day: u8) -> IsoDate {
        IsoDate { year, month, day }
    }

    pub fn at(self, time: Time) -> DateTime {
        DateTime { date: self, time }
    }
}

impl Time {
    /// Nanossegundos desde a meia-noite (0 ≤ n < 1 dia).
    pub fn from_ns(n: i64) -> Time {
        Time {
            hour: (n / 3_600_000_000_000) as u8,
            minute: (n / 60_000_000_000 % 60) as u8,
            second: (n / 1_000_000_000 % 60) as u8,
            ms: (n / 1_000_000 % 1000) as u16,
            us: (n / 1000 % 1000) as u16,
            ns: (n % 1000) as u16,
        }
    }

    pub fn total_ns(&self) -> i64 {
        ((self.hour as i64 * 60 + self.minute as i64) * 60 + self.second as i64) * 1_000_000_000
            + self.ms as i64 * 1_000_000
            + self.us as i64 * 1000
            + self.ns as i64
    }

    /// Fração de segundo em ns.
    pub fn subsecond_ns(&self) -> u32 {
        self.ms as u32 * 1_000_000 + self.us as u32 * 1000 + self.ns as u32
    }
}

pub fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

pub fn days_in_month(y: i64, m: u8) -> u8 {
    match m {
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub fn days_in_year(y: i64) -> u16 {
    if is_leap(y) { 366 } else { 365 }
}

pub fn is_valid_date(y: i64, m: i64, d: i64) -> bool {
    (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m as u8) as i64
}

pub fn is_valid_time(h: i64, mi: i64, s: i64, ms: i64, us: i64, ns: i64) -> bool {
    (0..=23).contains(&h)
        && (0..=59).contains(&mi)
        && (0..=59).contains(&s)
        && (0..=999).contains(&ms)
        && (0..=999).contains(&us)
        && (0..=999).contains(&ns)
}

/// Dias desde 1970-01-01 (o ano é limitado para não estourar; anos tão
/// grandes já estão muito fora do intervalo do Temporal).
pub fn epoch_days(y: i64, m: u8, d: u8) -> i64 {
    let y = y.clamp(-1_000_000_000_000, 1_000_000_000_000);
    let (m, d) = (m as i64, d as i64);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn date_epoch_days(d: &IsoDate) -> i64 {
    epoch_days(d.year, d.month, d.day)
}

pub fn from_epoch_days(z: i64) -> IsoDate {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    IsoDate {
        year: if m <= 2 { y + 1 } else { y },
        month: m as u8,
        day: d as u8,
    }
}

/// AddDaysToISODate.
pub fn add_days(d: IsoDate, days: i64) -> IsoDate {
    if days == 0 {
        return d;
    }
    from_epoch_days(date_epoch_days(&d).saturating_add(days))
}

/// BalanceISOYearMonth: mês qualquer → (ano, 1..12).
pub fn balance_year_month(y: i64, m: i64) -> (i64, u8) {
    let m0 = m - 1;
    (y + m0.div_euclid(12), (m0.rem_euclid(12) + 1) as u8)
}

/// ConstrainISODate (mês e dia já limitados por quem chama ou não).
pub fn constrain_date(y: i64, m: i64, d: i64) -> IsoDate {
    let m = m.clamp(1, 12) as u8;
    let d = d.clamp(1, days_in_month(y, m) as i64) as u8;
    IsoDate::new(y, m, d)
}

/// RegulateISODate: None quando `reject` e a data não existe.
pub fn regulate_date(y: i64, m: i64, d: i64, constrain: bool) -> Option<IsoDate> {
    if constrain {
        return Some(constrain_date(y, m, d));
    }
    is_valid_date(y, m, d).then(|| IsoDate::new(y, m as u8, d as u8))
}

/// RegulateTime: None quando `reject` e a hora não existe.
pub fn regulate_time(h: i64, mi: i64, s: i64, ms: i64, us: i64, ns: i64, constrain: bool) -> Option<Time> {
    if constrain {
        return Some(Time {
            hour: h.clamp(0, 23) as u8,
            minute: mi.clamp(0, 59) as u8,
            second: s.clamp(0, 59) as u8,
            ms: ms.clamp(0, 999) as u16,
            us: us.clamp(0, 999) as u16,
            ns: ns.clamp(0, 999) as u16,
        });
    }
    is_valid_time(h, mi, s, ms, us, ns).then_some(Time {
        hour: h as u8,
        minute: mi as u8,
        second: s as u8,
        ms: ms as u16,
        us: us as u16,
        ns: ns as u16,
    })
}

/// BalanceTime: componentes quaisquer → (dias a mais, hora válida).
pub fn balance_time(h: i128, mi: i128, s: i128, ms: i128, us: i128, ns: i128) -> (i64, Time) {
    let total = ((((h * 60 + mi) * 60 + s) * 1000 + ms) * 1000 + us) * 1000 + ns;
    balance_ns(total)
}

/// Nanossegundos (podem passar de um dia ou ser negativos) → (dias, hora).
pub fn balance_ns(total: i128) -> (i64, Time) {
    let days = total.div_euclid(NS_PER_DAY);
    let rem = total.rem_euclid(NS_PER_DAY);
    (days as i64, Time::from_ns(rem as i64))
}

/// AddTime: hora + duração de tempo (ns).
pub fn add_time(t: &Time, dur: i128) -> (i64, Time) {
    balance_ns(t.total_ns() as i128 + dur)
}

/// BalanceISODateTime com um deslocamento em ns.
pub fn add_ns_to_date_time(dt: &DateTime, ns: i128) -> DateTime {
    let (days, time) = add_time(&dt.time, ns);
    DateTime {
        date: add_days(dt.date, days),
        time,
    }
}

/// GetUTCEpochNanoseconds.
pub fn epoch_ns(dt: &DateTime) -> i128 {
    date_epoch_days(&dt.date) as i128 * NS_PER_DAY + dt.time.total_ns() as i128
}

/// Data e hora UTC de um instante.
pub fn from_epoch_ns(ns: i128) -> DateTime {
    let days = ns.div_euclid(NS_PER_DAY);
    let rem = ns.rem_euclid(NS_PER_DAY);
    DateTime {
        date: from_epoch_days(days as i64),
        time: Time::from_ns(rem as i64),
    }
}

pub fn is_valid_epoch_ns(ns: i128) -> bool {
    (NS_MIN..=NS_MAX).contains(&ns)
}

/// ISODateTimeWithinLimits.
pub fn date_time_within_limits(dt: &DateTime) -> bool {
    if dt.date.year.abs() > 300_000 {
        return false;
    }
    let ns = epoch_ns(dt);
    ns > NS_MIN - NS_PER_DAY && ns < NS_MAX + NS_PER_DAY
}

/// ISODateWithinLimits (ao meio-dia).
pub fn date_within_limits(d: &IsoDate) -> bool {
    date_time_within_limits(&d.at(NOON))
}

/// ISOYearMonthWithinLimits.
pub fn year_month_within_limits(y: i64, m: u8) -> bool {
    if !(-271821..=275760).contains(&y) {
        return false;
    }
    !(y == -271821 && m < 4 || y == 275760 && m > 9)
}

/// CheckISODaysRange: |dias desde a época| ≤ 10^8.
pub fn iso_days_in_range(d: &IsoDate) -> bool {
    d.year.abs() < 300_000 && date_epoch_days(d).abs() <= 100_000_000
}

pub fn cmp_date(a: &IsoDate, b: &IsoDate) -> i32 {
    ord(a.cmp(b))
}

pub fn cmp_date_time(a: &DateTime, b: &DateTime) -> i32 {
    ord(a.cmp(b))
}

pub fn ord(o: Ordering) -> i32 {
    match o {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// Dia da semana ISO (segunda = 1 ... domingo = 7).
pub fn day_of_week(d: &IsoDate) -> u8 {
    ((date_epoch_days(d) + 3).rem_euclid(7) + 1) as u8
}

pub fn day_of_year(d: &IsoDate) -> u16 {
    (date_epoch_days(d) - epoch_days(d.year, 1, 1) + 1) as u16
}

fn weeks_in_year(y: i64) -> i64 {
    let jan1 = day_of_week(&IsoDate::new(y, 1, 1));
    if jan1 == 4 || (jan1 == 3 && is_leap(y)) { 53 } else { 52 }
}

/// Semana ISO 8601 e o ano a que ela pertence.
pub fn week_of_year(d: &IsoDate) -> (u8, i64) {
    let w = (day_of_year(d) as i64 - day_of_week(d) as i64 + 10).div_euclid(7);
    if w < 1 {
        (weeks_in_year(d.year - 1) as u8, d.year - 1)
    } else if w > weeks_in_year(d.year) {
        (1, d.year + 1)
    } else {
        (w as u8, d.year)
    }
}

// ------------------------------------------------------------ texto

/// ISOYearString: 4 dígitos ou ±6 dígitos.
pub fn year_string(y: i64) -> String {
    if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else {
        format!("{}{:06}", if y < 0 { '-' } else { '+' }, y.unsigned_abs())
    }
}

/// "YYYY-MM-DD".
pub fn date_string(d: &IsoDate) -> String {
    format!("{}-{:02}-{:02}", year_string(d.year), d.month, d.day)
}

/// FormatFractionalSeconds (precisão: None = auto, Some(n) = n dígitos).
pub fn fraction_string(sub_ns: u32, precision: Option<u8>) -> String {
    match precision {
        None => {
            if sub_ns == 0 {
                return String::new();
            }
            let f = format!("{sub_ns:09}");
            format!(".{}", f.trim_end_matches('0'))
        }
        Some(0) => String::new(),
        Some(p) => {
            let f = format!("{sub_ns:09}");
            format!(".{}", &f[..(p as usize).min(9)])
        }
    }
}

/// FormatTimeString (precisão Some(255) = só horas e minutos).
pub fn time_string(t: &Time, precision: Option<u8>) -> String {
    let hm = format!("{:02}:{:02}", t.hour, t.minute);
    if precision == Some(255) {
        return hm;
    }
    format!("{hm}:{:02}{}", t.second, fraction_string(t.subsecond_ns(), precision))
}

/// "YYYY-MM-DDTHH:MM..." (sem calendário).
pub fn date_time_string(dt: &DateTime, precision: Option<u8>) -> String {
    format!("{}T{}", date_string(&dt.date), time_string(&dt.time, precision))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dias() {
        assert_eq!(epoch_days(1970, 1, 1), 0);
        assert_eq!(from_epoch_days(0), IsoDate::new(1970, 1, 1));
        assert_eq!(from_epoch_days(-100_000_000), IsoDate::new(-271821, 4, 20));
        assert_eq!(from_epoch_days(100_000_000), IsoDate::new(275760, 9, 13));
        assert_eq!(add_days(IsoDate::new(2020, 2, 28), 1), IsoDate::new(2020, 2, 29));
        assert_eq!(day_of_week(&IsoDate::new(1970, 1, 1)), 4);
        assert_eq!(week_of_year(&IsoDate::new(2021, 1, 1)), (53, 2020));
        assert_eq!(week_of_year(&IsoDate::new(2019, 12, 30)), (1, 2020));
        assert_eq!(balance_year_month(2020, 0), (2019, 12));
        assert_eq!(balance_time(25, 0, 0, 0, 0, -1), (1, Time::from_ns(3_600_000_000_000 - 1)));
    }
}
