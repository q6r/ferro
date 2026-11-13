//! Fusos horários do Temporal: identificadores, deslocamentos e a conversão
//! entre instantes e hora local. Os fusos com nome vêm do hospedeiro
//! (`Vm::zones`); deslocamentos fixos e UTC são resolvidos aqui.

use std::rc::Rc;

use super::iso::{self, DateTime, IsoDate, MIDNIGHT, NS_PER_DAY, Time};
use super::parse::{self, TzId};
use super::{TemporalData, range, type_err};
use crate::value::*;
use crate::vm::Vm;

/// Fuso de um ZonedDateTime.
#[derive(Clone, Debug)]
pub enum Tz {
    /// Deslocamento fixo, em minutos.
    Offset(i32),
    /// Fuso com nome: identificador (grafia normalizada) e o primário.
    Named { id: Rc<str>, primary: Rc<str> },
}

impl Tz {
    pub fn utc() -> Tz {
        Tz::Named {
            id: "UTC".into(),
            primary: "UTC".into(),
        }
    }

    /// O identificador mostrado (timeZoneId, toString).
    pub fn id(&self) -> String {
        match self {
            Tz::Offset(m) => format_offset_minutes(*m),
            Tz::Named { id, .. } => id.to_string(),
        }
    }

    /// TimeZoneEquals.
    pub fn equals(&self, o: &Tz) -> bool {
        match (self, o) {
            (Tz::Offset(a), Tz::Offset(b)) => a == b,
            (Tz::Named { primary: a, .. }, Tz::Named { primary: b, .. }) => a == b,
            _ => false,
        }
    }

    /// Sem transições (deslocamento fixo ou UTC).
    pub fn is_fixed(&self) -> bool {
        match self {
            Tz::Offset(_) => true,
            Tz::Named { primary, .. } => &**primary == "UTC",
        }
    }
}

/// FormatOffsetTimeZoneIdentifier: "±HH:MM".
pub fn format_offset_minutes(m: i32) -> String {
    let sign = if m >= 0 { '+' } else { '-' };
    let a = m.unsigned_abs();
    format!("{sign}{:02}:{:02}", a / 60, a % 60)
}

/// FormatUTCOffsetNanoseconds: "±HH:MM", "±HH:MM:SS" ou com fração.
pub fn format_offset_ns(ns: i64) -> String {
    let sign = if ns >= 0 { '+' } else { '-' };
    let a = ns.unsigned_abs();
    let (h, m, s, sub) = (a / 3_600_000_000_000, a / 60_000_000_000 % 60, a / 1_000_000_000 % 60, a % 1_000_000_000);
    let mut out = format!("{sign}{h:02}:{m:02}");
    if s != 0 || sub != 0 {
        out.push_str(&format!(":{s:02}"));
        if sub != 0 {
            let f = format!("{sub:09}");
            out.push('.');
            out.push_str(f.trim_end_matches('0'));
        }
    }
    out
}

/// FormatDateTimeUTCOffsetRounded: arredonda ao minuto (meio para longe de zero).
pub fn format_offset_rounded(ns: i64) -> String {
    let m = super::round::round_i128(ns as i128, 60_000_000_000, super::round::RoundingMode::HalfExpand) / 60_000_000_000;
    format_offset_minutes(m as i32)
}

/// ToTemporalTimeZoneIdentifier.
pub fn to_time_zone(vm: &mut Vm, v: &Value) -> Result<Tz, Value> {
    if let Some(TemporalData::Zoned(_, tz, _)) = super::data(v) {
        return Ok(tz);
    }
    let Value::String(s) = v else {
        let d = vm.describe(v);
        return type_err(vm, format!("Invalid time zone: {d} is not a string"));
    };
    from_string(vm, &s.to_rust())
}

/// ParseTemporalTimeZoneString + nome disponível.
pub fn from_string(vm: &mut Vm, s: &str) -> Result<Tz, Value> {
    let id = match parse::parse_tz_identifier(s) {
        Some(id) => id,
        None => {
            let p = match parse::parse_iso(s, &parse::ALL_GOALS) {
                Ok(p) => p,
                Err(_) => return range(vm, format!("Invalid time zone: {s}")),
            };
            if let Some(a) = p.tz {
                parse::parse_tz_identifier(&a).unwrap_or(TzId::Name(a))
            } else if p.z {
                TzId::Name("UTC".into())
            } else if let Some(o) = p.offset {
                match parse::parse_tz_identifier(&o) {
                    Some(id) => id,
                    None => return range(vm, format!("Invalid time zone: seconds not allowed in offset time zone {o}")),
                }
            } else {
                return range(vm, format!("Invalid time zone: {s}"));
            }
        }
    };
    match id {
        TzId::Offset(m) => Ok(Tz::Offset(m)),
        TzId::Name(n) => resolve_name(vm, &n),
    }
}

/// GetAvailableNamedTimeZoneIdentifier (RangeError se não existir).
pub fn resolve_name(vm: &mut Vm, name: &str) -> Result<Tz, Value> {
    match vm.zones.resolve(name) {
        Some((id, primary)) => Ok(Tz::Named {
            id: id.into(),
            primary: primary.into(),
        }),
        None => range(vm, format!("Invalid time zone: {name}")),
    }
}

/// GetOffsetNanosecondsFor.
pub fn offset_ns(vm: &Vm, tz: &Tz, epoch: i128) -> i64 {
    match tz {
        Tz::Offset(m) => *m as i64 * 60_000_000_000,
        Tz::Named { id, primary } => {
            if &**primary == "UTC" {
                0
            } else {
                vm.zones.offset_ns(id, epoch)
            }
        }
    }
}

/// GetISODateTimeFor.
pub fn date_time_for(vm: &Vm, tz: &Tz, epoch: i128) -> DateTime {
    iso::from_epoch_ns(epoch + offset_ns(vm, tz, epoch) as i128)
}

/// GetPossibleEpochNanoseconds.
pub fn possible(vm: &mut Vm, tz: &Tz, dt: &DateTime) -> Result<Vec<i128>, Value> {
    let list = match tz {
        Tz::Offset(m) => {
            let b = iso::add_ns_to_date_time(dt, -(*m as i128) * 60_000_000_000);
            if !iso::iso_days_in_range(&b.date) {
                return range(vm, "Date and time are outside of supported range");
            }
            vec![iso::epoch_ns(&b)]
        }
        Tz::Named { id, primary } => {
            if &**primary == "UTC" {
                if !iso::iso_days_in_range(&dt.date) {
                    return range(vm, "Date and time are outside of supported range");
                }
                vec![iso::epoch_ns(dt)]
            } else {
                if dt.date.year.abs() > 300_000 {
                    return range(vm, "Date and time are outside of supported range");
                }
                vm.zones.possible(id, iso::epoch_ns(dt))
            }
        }
    };
    if list.iter().any(|&n| !iso::is_valid_epoch_ns(n)) {
        return range(vm, "Date and time are outside of supported range");
    }
    Ok(list)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disambig {
    Compatible,
    Earlier,
    Later,
    Reject,
}

/// DisambiguatePossibleEpochNanoseconds.
pub fn disambiguate(vm: &mut Vm, list: &[i128], tz: &Tz, dt: &DateTime, dis: Disambig) -> Result<i128, Value> {
    let n = list.len();
    if n == 1 {
        return Ok(list[0]);
    }
    if n != 0 {
        return match dis {
            Disambig::Earlier | Disambig::Compatible => Ok(list[0]),
            Disambig::Later => Ok(list[n - 1]),
            Disambig::Reject => range(vm, "Ambiguous local time: multiple instants found"),
        };
    }
    if dis == Disambig::Reject {
        return range(vm, "Local time does not exist in the time zone (skipped by a transition)");
    }
    let utc = iso::epoch_ns(dt);
    let before = utc - NS_PER_DAY;
    let after = utc + NS_PER_DAY;
    if !iso::is_valid_epoch_ns(before) || !iso::is_valid_epoch_ns(after) {
        return range(vm, "Date and time are outside of supported range");
    }
    let shift = (offset_ns(vm, tz, after) - offset_ns(vm, tz, before)) as i128;
    if dis == Disambig::Earlier {
        let earlier = iso::add_ns_to_date_time(dt, -shift);
        let p = possible(vm, tz, &earlier)?;
        return match p.first() {
            Some(&x) => Ok(x),
            None => range(vm, "Local time does not exist in the time zone"),
        };
    }
    let later = iso::add_ns_to_date_time(dt, shift);
    let p = possible(vm, tz, &later)?;
    match p.last() {
        Some(&x) => Ok(x),
        None => range(vm, "Local time does not exist in the time zone"),
    }
}

/// GetEpochNanosecondsFor.
pub fn epoch_for(vm: &mut Vm, tz: &Tz, dt: &DateTime, dis: Disambig) -> Result<i128, Value> {
    let list = possible(vm, tz, dt)?;
    disambiguate(vm, &list, tz, dt, dis)
}

/// GetStartOfDay.
pub fn start_of_day(vm: &mut Vm, tz: &Tz, date: &IsoDate) -> Result<i128, Value> {
    let dt = date.at(MIDNIGHT);
    let list = possible(vm, tz, &dt)?;
    if let Some(&x) = list.first() {
        return Ok(x);
    }
    // meia-noite pulada: o primeiro instante depois da transição
    let before = iso::epoch_ns(&dt) - NS_PER_DAY;
    if !iso::is_valid_epoch_ns(before) {
        return range(vm, "Date and time are outside of supported range");
    }
    match transition(vm, tz, before, true) {
        Some(t) => Ok(t),
        None => range(vm, "Local time does not exist in the time zone"),
    }
}

/// GetNamedTimeZoneNextTransition / PreviousTransition (None nos fusos fixos).
pub fn transition(vm: &Vm, tz: &Tz, epoch: i128, next: bool) -> Option<i128> {
    match tz {
        Tz::Named { id, .. } if !tz.is_fixed() => {
            let t = vm.zones.transition(id, epoch, next)?;
            iso::is_valid_epoch_ns(t).then_some(t)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OffsetBehaviour {
    Option,
    Exact,
    Wall,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OffsetOption {
    Prefer,
    Use,
    Ignore,
    Reject,
}

/// InterpretISODateTimeOffset (`time` None: ~start-of-day~).
#[allow(clippy::too_many_arguments)]
pub fn interpret_offset(
    vm: &mut Vm,
    date: IsoDate,
    time: Option<Time>,
    behaviour: OffsetBehaviour,
    offset: i128,
    tz: &Tz,
    dis: Disambig,
    opt: OffsetOption,
    match_minutes: bool,
) -> Result<i128, Value> {
    let Some(time) = time else {
        return start_of_day(vm, tz, &date);
    };
    let dt = date.at(time);
    if behaviour == OffsetBehaviour::Wall || (behaviour == OffsetBehaviour::Option && opt == OffsetOption::Ignore) {
        return epoch_for(vm, tz, &dt, dis);
    }
    if behaviour == OffsetBehaviour::Exact || opt == OffsetOption::Use {
        let b = iso::add_ns_to_date_time(&dt, -offset);
        if !iso::iso_days_in_range(&b.date) {
            return range(vm, "Date and time are outside of supported range");
        }
        let ns = iso::epoch_ns(&b);
        if !iso::is_valid_epoch_ns(ns) {
            return range(vm, "Date and time are outside of supported range");
        }
        return Ok(ns);
    }
    if !iso::iso_days_in_range(&date) {
        return range(vm, "Date and time are outside of supported range");
    }
    let utc = iso::epoch_ns(&dt);
    let list = possible(vm, tz, &dt)?;
    for &c in &list {
        let cand = utc - c;
        if cand == offset {
            return Ok(c);
        }
        if match_minutes
            && super::round::round_i128(cand, 60_000_000_000, super::round::RoundingMode::HalfExpand) == offset
        {
            return Ok(c);
        }
    }
    if opt == OffsetOption::Reject {
        let o = format_offset_ns(offset as i64);
        return range(vm, format!("Offset {o} is invalid for the given date and time in {}", tz.id()));
    }
    disambiguate(vm, &list, tz, &dt, dis)
}

/// SystemTimeZoneIdentifier como fuso.
pub fn system(vm: &mut Vm) -> Tz {
    let id = vm.zones.system();
    if let Some(TzId::Offset(m)) = parse::parse_tz_identifier(&id) {
        return Tz::Offset(m);
    }
    match vm.zones.resolve(&id) {
        Some((id, primary)) => Tz::Named {
            id: id.into(),
            primary: primary.into(),
        },
        None => Tz::utc(),
    }
}
