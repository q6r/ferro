//! Análise das strings ISO 8601 / RFC 9557 do Temporal, seguindo a gramática
//! do spec (sem expressões regulares).

use super::iso::{self, Time};

/// Meta da gramática (o formato esperado da string).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Goal {
    /// TemporalDateTimeString[~Zoned]
    DateTime,
    /// TemporalDateTimeString[+Zoned]
    Zoned,
    Instant,
    Time,
    MonthDay,
    YearMonth,
}

/// ISO Date-Time Parse Record.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    /// None: string de mês-dia sem ano.
    pub year: Option<i64>,
    pub month: u8,
    pub day: u8,
    /// None: ~start-of-day~ (sem hora na string).
    pub time: Option<Time>,
    pub z: bool,
    pub offset: Option<String>,
    pub tz: Option<String>,
    pub calendar: Option<String>,
}

/// Resultado sintático, antes de interpretar as anotações.
#[derive(Default)]
struct Raw {
    year: Option<i64>,
    month: u8,
    day: u8,
    has_day: bool,
    time: Option<Time>,
    z: bool,
    offset: Option<String>,
    tz: Option<String>,
    anns: Vec<(bool, String, String)>,
}

struct Cur<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn new(s: &'a str) -> Cur<'a> {
        Cur { s, b: s.as_bytes(), i: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn digit_at(&self, k: usize) -> bool {
        self.b.get(self.i + k).is_some_and(|c| c.is_ascii_digit())
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn eat_ci(&mut self, c: u8) -> bool {
        if self.peek().map(|x| x.to_ascii_uppercase()) == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    /// Um ou mais dígitos.
    fn digits(&mut self) -> Option<&'a str> {
        let start = self.i;
        while self.digit_at(0) {
            self.i += 1;
        }
        (self.i > start).then(|| &self.s[start..self.i])
    }

    fn end(&self) -> bool {
        self.i >= self.b.len()
    }

    /// Exatamente `n` dígitos.
    fn num(&mut self, n: usize) -> Option<u64> {
        if self.i + n > self.b.len() {
            return None;
        }
        let mut v = 0u64;
        for k in 0..n {
            let c = self.b[self.i + k];
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (c - b'0') as u64;
        }
        self.i += n;
        Some(v)
    }

    fn two(&mut self, max: u64) -> Option<u8> {
        let v = self.num(2)?;
        (v <= max).then_some(v as u8)
    }

    /// TemporalDecimalFraction opcional: Some(None) se ausente, None se inválida.
    /// O valor vem em ns (dígitos completados até 9).
    fn fraction(&mut self) -> Option<Option<u32>> {
        if !matches!(self.peek(), Some(b'.' | b',')) {
            return Some(None);
        }
        self.i += 1;
        let start = self.i;
        while self.digit_at(0) {
            self.i += 1;
        }
        let n = self.i - start;
        if n == 0 || n > 9 {
            return None;
        }
        let mut v: u32 = self.s[start..self.i].parse().ok()?;
        for _ in n..9 {
            v *= 10;
        }
        Some(Some(v))
    }
}

/// DateYear (o ano "-000000" é proibido).
fn date_year(c: &mut Cur) -> Option<i64> {
    match c.peek()? {
        s @ (b'+' | b'-') => {
            c.i += 1;
            let v = c.num(6)? as i64;
            if s == b'-' {
                if v == 0 {
                    return None;
                }
                Some(-v)
            } else {
                Some(v)
            }
        }
        _ => Some(c.num(4)? as i64),
    }
}

fn month(c: &mut Cur) -> Option<u8> {
    let m = c.two(12)?;
    (m >= 1).then_some(m)
}

fn day(c: &mut Cur) -> Option<u8> {
    let d = c.two(31)?;
    (d >= 1).then_some(d)
}

/// Date: DateSpec[+Extended] ou DateSpec[~Extended], com a data válida.
fn date(c: &mut Cur) -> Option<(i64, u8, u8)> {
    let y = date_year(c)?;
    let ext = c.eat(b'-');
    let m = month(c)?;
    if ext && !c.eat(b'-') {
        return None;
    }
    let d = day(c)?;
    iso::is_valid_date(y, m as i64, d as i64).then_some((y, m, d))
}

/// Time (TimeSpec estendido ou básico); segundo 60 vira 59.
fn time(c: &mut Cur) -> Option<Time> {
    let hour = c.two(23)?;
    let (mut minute, mut second, mut frac) = (0, 0, 0);
    if c.peek() == Some(b':') {
        c.i += 1;
        minute = c.two(59)?;
        if c.peek() == Some(b':') {
            c.i += 1;
            second = c.two(60)?;
            frac = c.fraction()?.unwrap_or(0);
        }
    } else if c.digit_at(0) && c.digit_at(1) {
        minute = c.two(59)?;
        if c.digit_at(0) && c.digit_at(1) {
            second = c.two(60)?;
            frac = c.fraction()?.unwrap_or(0);
        }
    }
    Some(Time {
        hour,
        minute,
        second: second.min(59),
        ms: (frac / 1_000_000) as u16,
        us: (frac / 1000 % 1000) as u16,
        ns: (frac % 1000) as u16,
    })
}

/// UTCOffset[SubMinutePrecision] → (ns, tinha segundos).
fn utc_offset(c: &mut Cur, sub_minute: bool) -> Option<(i128, bool)> {
    let sign: i128 = match c.peek()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    c.i += 1;
    let h = c.two(23)? as i128;
    let (mut m, mut s, mut f, mut has_sec) = (0i128, 0i128, 0i128, false);
    if c.peek() == Some(b':') {
        c.i += 1;
        m = c.two(59)? as i128;
        if sub_minute && c.peek() == Some(b':') {
            c.i += 1;
            s = c.two(59)? as i128;
            has_sec = true;
            f = c.fraction()?.unwrap_or(0) as i128;
        }
    } else if c.digit_at(0) && c.digit_at(1) {
        m = c.two(59)? as i128;
        if sub_minute && c.digit_at(0) && c.digit_at(1) {
            s = c.two(59)? as i128;
            has_sec = true;
            f = c.fraction()?.unwrap_or(0) as i128;
        }
    }
    Some((sign * ((h * 3600 + m * 60 + s) * 1_000_000_000 + f), has_sec))
}

fn is_tz_leading(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'.' || c == b'_'
}

fn is_tz_char(c: u8) -> bool {
    is_tz_leading(c) || c.is_ascii_digit() || c == b'-' || c == b'+'
}

/// TimeZoneIANAName.
fn is_iana_name(s: &str) -> bool {
    !s.is_empty()
        && s.split('/').all(|part| {
            let b = part.as_bytes();
            !b.is_empty() && is_tz_leading(b[0]) && b[1..].iter().all(|&c| is_tz_char(c))
        })
}

/// Identificador de fuso já analisado.
#[derive(Clone, Debug, PartialEq)]
pub enum TzId {
    Offset(i32),
    Name(String),
}

/// ParseTimeZoneIdentifier (None: erro de sintaxe).
pub fn parse_tz_identifier(s: &str) -> Option<TzId> {
    if matches!(s.as_bytes().first(), Some(b'+' | b'-')) {
        let mut c = Cur::new(s);
        let (ns, _) = utc_offset(&mut c, false)?;
        return c.end().then_some(TzId::Offset((ns / 60_000_000_000) as i32));
    }
    is_iana_name(s).then(|| TzId::Name(s.to_string()))
}

fn is_annotation_key(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b[1..].iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// AnnotationValue: componentes alfanuméricos separados por "-".
pub fn is_annotation_value(s: &str) -> bool {
    !s.is_empty() && s.split('-').all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_alphanumeric()))
}

/// TimeZoneAnnotation? Annotations? até o fim da string.
fn annotations(c: &mut Cur, r: &mut Raw) -> Option<()> {
    while c.peek() == Some(b'[') {
        let rest = &c.s[c.i + 1..];
        let close = rest.find(']')?;
        let content = &rest[..close];
        let body = content.strip_prefix('!').unwrap_or(content);
        let critical = body.len() != content.len();
        if let Some(eq) = body.find('=') {
            let (key, value) = (&body[..eq], &body[eq + 1..]);
            if !is_annotation_key(key) || !is_annotation_value(value) {
                return None;
            }
            r.anns.push((critical, key.to_string(), value.to_string()));
        } else {
            // o fuso vem antes das outras anotações, e só um
            if r.tz.is_some() || !r.anns.is_empty() {
                return None;
            }
            parse_tz_identifier(body)?;
            r.tz = Some(body.to_string());
        }
        c.i += close + 2;
    }
    c.end().then_some(())
}

/// AnnotatedDateTime (superconjunto: Z, deslocamento e fuso opcionais).
fn date_time_form(s: &str) -> Option<Raw> {
    let mut c = Cur::new(s);
    let (y, m, d) = date(&mut c)?;
    let mut r = Raw {
        year: Some(y),
        month: m,
        day: d,
        has_day: true,
        ..Default::default()
    };
    if matches!(c.peek(), Some(b' ' | b'T' | b't')) {
        c.i += 1;
        r.time = Some(time(&mut c)?);
        match c.peek() {
            Some(b'Z' | b'z') => {
                c.i += 1;
                r.z = true;
            }
            Some(b'+' | b'-') => {
                let start = c.i;
                utc_offset(&mut c, true)?;
                r.offset = Some(s[start..c.i].to_string());
            }
            _ => {}
        }
    }
    annotations(&mut c, &mut r)?;
    Some(r)
}

/// DateSpecMonthDay inteiro (com a validade de mês e dia).
fn is_month_day(s: &str) -> bool {
    let mut c = Cur::new(s);
    if s.starts_with("--") {
        c.i = 2;
    }
    let Some(m) = month(&mut c) else { return false };
    c.eat(b'-');
    let Some(d) = day(&mut c) else { return false };
    c.end() && iso::is_valid_date(1972, m as i64, d as i64)
}

/// DateSpecYearMonth inteiro.
fn is_year_month(s: &str) -> bool {
    let mut c = Cur::new(s);
    if date_year(&mut c).is_none() {
        return false;
    }
    c.eat(b'-');
    month(&mut c).is_some() && c.end()
}

/// AnnotatedTime (sem data); sem o T, não pode ser confundido com mês-dia
/// ou ano-mês.
fn time_form(s: &str) -> Option<Raw> {
    let mut c = Cur::new(s);
    let designator = c.eat(b'T') || c.eat(b't');
    let start = c.i;
    let mut r = Raw {
        time: Some(time(&mut c)?),
        ..Default::default()
    };
    if matches!(c.peek(), Some(b'+' | b'-')) {
        let o = c.i;
        utc_offset(&mut c, true)?;
        r.offset = Some(s[o..c.i].to_string());
    }
    let end = c.i;
    annotations(&mut c, &mut r)?;
    if !designator && (is_month_day(&s[start..end]) || is_year_month(&s[start..end])) {
        return None;
    }
    Some(r)
}

/// AnnotatedMonthDay.
fn month_day_form(s: &str) -> Option<Raw> {
    let mut c = Cur::new(s);
    if s.starts_with("--") {
        c.i = 2;
    }
    let m = month(&mut c)?;
    c.eat(b'-');
    let d = day(&mut c)?;
    if !iso::is_valid_date(1972, m as i64, d as i64) {
        return None;
    }
    let mut r = Raw {
        month: m,
        day: d,
        has_day: true,
        ..Default::default()
    };
    annotations(&mut c, &mut r)?;
    Some(r)
}

/// AnnotatedYearMonth.
fn year_month_form(s: &str) -> Option<Raw> {
    let mut c = Cur::new(s);
    let y = date_year(&mut c)?;
    c.eat(b'-');
    let m = month(&mut c)?;
    let mut r = Raw {
        year: Some(y),
        month: m,
        day: 1,
        ..Default::default()
    };
    annotations(&mut c, &mut r)?;
    Some(r)
}

/// Análise só sintática com uma meta.
fn try_goal(s: &str, goal: Goal) -> Option<Raw> {
    match goal {
        Goal::DateTime => date_time_form(s).filter(|r| !r.z),
        Goal::Zoned => date_time_form(s).filter(|r| r.tz.is_some()),
        Goal::Instant => date_time_form(s).filter(|r| r.time.is_some() && (r.z || r.offset.is_some())),
        Goal::Time => time_form(s).or_else(|| date_time_form(s).filter(|r| r.time.is_some() && !r.z)),
        Goal::MonthDay => month_day_form(s).or_else(|| date_time_form(s).filter(|r| !r.z)),
        Goal::YearMonth => year_month_form(s).or_else(|| date_time_form(s).filter(|r| !r.z)),
    }
}

/// ParseISODateTime com as metas dadas (a primeira que analisar vale).
pub fn parse_iso(s: &str, goals: &[Goal]) -> Result<Parsed, String> {
    for &goal in goals {
        let Some(r) = try_goal(s, goal) else { continue };
        let mut calendar: Option<String> = None;
        let mut critical_cal = false;
        for (critical, key, value) in &r.anns {
            if key == "u-ca" {
                if calendar.is_none() {
                    calendar = Some(value.clone());
                    critical_cal = *critical;
                } else if *critical || critical_cal {
                    return Err(format!("Invalid annotations in {s}: more than one u-ca present with critical flag"));
                }
            } else if *critical {
                return Err(format!("Unrecognized annotation: !{key}={value}"));
            }
        }
        let short_form = match goal {
            Goal::YearMonth => !r.has_day,
            Goal::MonthDay => r.year.is_none(),
            _ => false,
        };
        if short_form && calendar.as_ref().is_some_and(|c| !c.eq_ignore_ascii_case("iso8601")) {
            return Err(format!("Calendar annotation is only valid with iso8601 in {s}"));
        }
        return Ok(Parsed {
            year: r.year,
            month: r.month,
            day: r.day,
            time: r.time,
            z: r.z,
            offset: r.offset,
            tz: r.tz,
            calendar,
        });
    }
    Err(format!("Invalid ISO 8601 string: {s}"))
}

/// Todas as metas, na ordem de ParseTemporalCalendarString/TimeZoneString.
pub const ALL_GOALS: [Goal; 6] = [Goal::Zoned, Goal::DateTime, Goal::Instant, Goal::Time, Goal::MonthDay, Goal::YearMonth];

/// ParseDateTimeUTCOffset: deslocamento em ns (None: string inválida).
pub fn parse_utc_offset(s: &str) -> Option<i128> {
    let mut c = Cur::new(s);
    let (ns, _) = utc_offset(&mut c, true)?;
    c.end().then_some(ns)
}

/// O deslocamento tem segundos (ou fração)?
pub fn offset_has_seconds(s: &str) -> bool {
    let mut c = Cur::new(s);
    utc_offset(&mut c, true).is_some_and(|(_, sec)| sec)
}

/// IsOffsetTimeZoneIdentifier.
pub fn is_offset_identifier(s: &str) -> bool {
    matches!(parse_tz_identifier(s), Some(TzId::Offset(_)))
}

/// ParseTemporalDurationString: os dez campos (ainda sem validar).
pub fn parse_duration(s: &str) -> Option<[f64; 10]> {
    let mut c = Cur::new(s);
    let sign = if c.eat(b'-') {
        -1.0
    } else {
        c.eat(b'+');
        1.0
    };
    if !c.eat_ci(b'P') {
        return None;
    }
    let mut date: [Option<f64>; 4] = [None; 4];
    let mut next = 0;
    let mut any = false;
    while c.digit_at(0) {
        let n: f64 = c.digits()?.parse().ok()?;
        let k = match c.peek()?.to_ascii_uppercase() {
            b'Y' => 0,
            b'M' => 1,
            b'W' => 2,
            b'D' => 3,
            _ => return None,
        };
        if k < next {
            return None;
        }
        c.i += 1;
        date[k] = Some(n);
        next = k + 1;
        any = true;
    }
    // horas, minutos e segundos; só a última pode ter fração
    let mut time: [Option<f64>; 3] = [None; 3];
    let mut frac: Option<(usize, u64)> = None;
    if c.eat_ci(b'T') {
        let mut next = 0;
        let mut any_time = false;
        while c.digit_at(0) {
            if frac.is_some() {
                return None;
            }
            let n: f64 = c.digits()?.parse().ok()?;
            let f = c.fraction()?;
            let k = match c.peek()?.to_ascii_uppercase() {
                b'H' => 0,
                b'M' => 1,
                b'S' => 2,
                _ => return None,
            };
            if k < next {
                return None;
            }
            c.i += 1;
            time[k] = Some(n);
            if let Some(f) = f {
                frac = Some((k, f as u64));
            }
            next = k + 1;
            any_time = true;
        }
        if !any_time {
            return None;
        }
        any = true;
    }
    if !any || !c.end() {
        return None;
    }
    // fração da menor unidade → ns a mais (exato)
    let excess: u64 = match frac {
        Some((0, f)) => f * 3600,
        Some((1, f)) => f * 60,
        Some((_, f)) => f,
        None => 0,
    };
    let v = |x: Option<f64>| x.unwrap_or(0.0);
    let mut minutes = v(time[1]);
    let mut seconds = v(time[2]);
    minutes += (excess / 60_000_000_000) as f64;
    seconds += (excess / 1_000_000_000 % 60) as f64;
    let ms = (excess / 1_000_000 % 1000) as f64;
    let us = (excess / 1000 % 1000) as f64;
    let ns = (excess % 1000) as f64;
    let f = [v(date[0]), v(date[1]), v(date[2]), v(date[3]), v(time[0]), minutes, seconds, ms, us, ns];
    Some(f.map(|x| x * sign + 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datas() {
        let p = parse_iso("2020-01-02T03:04:05.123456789+01:00[Europe/Paris][u-ca=iso8601]", &[Goal::Zoned]).unwrap();
        assert_eq!((p.year, p.month, p.day), (Some(2020), 1, 2));
        assert_eq!(p.time.unwrap().ns, 789);
        assert_eq!(p.offset.as_deref(), Some("+01:00"));
        assert_eq!(p.tz.as_deref(), Some("Europe/Paris"));
        assert_eq!(p.calendar.as_deref(), Some("iso8601"));
        assert!(parse_iso("2020-01-01T00:00Z", &[Goal::DateTime]).is_err());
        assert!(parse_iso("-000000-01-01", &[Goal::DateTime]).is_err());
        assert!(parse_iso("1214", &[Goal::Time]).is_err());
        assert!(parse_iso("T1214", &[Goal::Time]).is_ok());
        assert!(parse_iso("1232", &[Goal::Time]).is_ok());
        assert!(parse_iso("2021-12", &[Goal::Time]).is_err());
        assert!(parse_iso("2020-01-01[!foo=bar]", &[Goal::DateTime]).is_err());
        assert!(parse_iso("--10-01", &[Goal::MonthDay]).is_ok());
        assert!(parse_iso("1976-11[u-ca=gregory]", &[Goal::YearMonth]).is_err());
        assert_eq!(parse_utc_offset("-00:00:01.5"), Some(-1_500_000_000));
    }

    #[test]
    fn duracoes() {
        assert_eq!(parse_duration("P1Y2M3W4DT5H6M7.008009010S"), Some([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]));
        assert_eq!(parse_duration("-PT1.5H"), Some([0.0, 0.0, 0.0, 0.0, -1.0, -30.0, 0.0, 0.0, 0.0, 0.0]));
        assert!(parse_duration("P").is_none());
        assert!(parse_duration("PT").is_none());
        assert!(parse_duration("PT1.5H1M").is_none());
        assert!(parse_duration("P1D1Y").is_none());
    }
}
