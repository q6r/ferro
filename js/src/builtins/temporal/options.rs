//! Leitura das opções, na ordem observável do spec (cada Get seguido da
//! sua conversão).

use super::calendar::ShowCalendar;
use super::round::{RoundingMode, UNITS, Unit};
use super::zone::{Disambig, OffsetOption};
use super::{range, to_int_trunc, type_err};
use crate::value::*;
use crate::vm::Vm;

/// Objeto de opções (None: `undefined`, nenhuma leitura observável).
pub type Opts = Option<ObjRef>;

/// GetOptionsObject.
pub fn get_options(vm: &mut Vm, v: &Value) -> Result<Opts, Value> {
    match v {
        Value::Undefined => Ok(None),
        Value::Object(o) => Ok(Some(o.clone())),
        _ => {
            let d = vm.describe(v);
            type_err(vm, format!("Options parameter must be an object, not {d}"))
        }
    }
}

pub fn get(vm: &mut Vm, o: &Opts, key: &str) -> Result<Value, Value> {
    match o {
        Some(o) => vm.get_obj(o, &PropKey::str(key)),
        None => Ok(Value::Undefined),
    }
}

/// GetOption com tipo string: Some(valor) ou None se ausente.
fn string_option(vm: &mut Vm, o: &Opts, key: &str, allowed: &[&str]) -> Result<Option<String>, Value> {
    let v = get(vm, o, key)?;
    if v.is_undefined() {
        return Ok(None);
    }
    let s = vm.to_rust_string(&v)?;
    if !allowed.contains(&s.as_str()) {
        return range(vm, format!("Value {s} out of range for options property {key}"));
    }
    Ok(Some(s))
}

/// GetTemporalOverflowOption: true = constrain.
pub fn overflow(vm: &mut Vm, o: &Opts) -> Result<bool, Value> {
    Ok(string_option(vm, o, "overflow", &["constrain", "reject"])?.is_none_or(|s| s == "constrain"))
}

pub fn disambiguation(vm: &mut Vm, o: &Opts) -> Result<Disambig, Value> {
    Ok(match string_option(vm, o, "disambiguation", &["compatible", "earlier", "later", "reject"])?.as_deref() {
        Some("earlier") => Disambig::Earlier,
        Some("later") => Disambig::Later,
        Some("reject") => Disambig::Reject,
        _ => Disambig::Compatible,
    })
}

pub fn offset_option(vm: &mut Vm, o: &Opts, fallback: OffsetOption) -> Result<OffsetOption, Value> {
    Ok(match string_option(vm, o, "offset", &["prefer", "use", "ignore", "reject"])?.as_deref() {
        Some("prefer") => OffsetOption::Prefer,
        Some("use") => OffsetOption::Use,
        Some("ignore") => OffsetOption::Ignore,
        Some("reject") => OffsetOption::Reject,
        _ => fallback,
    })
}

pub fn rounding_mode(vm: &mut Vm, o: &Opts, fallback: RoundingMode) -> Result<RoundingMode, Value> {
    const ALL: [&str; 9] = ["ceil", "floor", "expand", "trunc", "halfCeil", "halfFloor", "halfExpand", "halfTrunc", "halfEven"];
    Ok(match string_option(vm, o, "roundingMode", &ALL)?.as_deref() {
        Some("ceil") => RoundingMode::Ceil,
        Some("floor") => RoundingMode::Floor,
        Some("expand") => RoundingMode::Expand,
        Some("trunc") => RoundingMode::Trunc,
        Some("halfCeil") => RoundingMode::HalfCeil,
        Some("halfFloor") => RoundingMode::HalfFloor,
        Some("halfExpand") => RoundingMode::HalfExpand,
        Some("halfTrunc") => RoundingMode::HalfTrunc,
        Some("halfEven") => RoundingMode::HalfEven,
        _ => fallback,
    })
}

/// GetRoundingIncrementOption: 1 a 10^9.
pub fn rounding_increment(vm: &mut Vm, o: &Opts) -> Result<i64, Value> {
    let v = get(vm, o, "roundingIncrement")?;
    if v.is_undefined() {
        return Ok(1);
    }
    let n = to_int_trunc(vm, &v, "roundingIncrement")?;
    if !(1.0..=1e9).contains(&n) {
        return range(vm, format!("roundingIncrement value is out of range: {}", crate::conv::number_to_string(n)));
    }
    Ok(n as i64)
}

/// ValidateTemporalRoundingIncrement.
pub fn validate_increment(vm: &mut Vm, inc: i64, dividend: i64, inclusive: bool) -> Result<(), Value> {
    let max = if inclusive { dividend } else { dividend - 1 };
    if inc > max {
        return range(vm, format!("roundingIncrement value is out of range: {inc}"));
    }
    if dividend % inc != 0 {
        return range(vm, format!("roundingIncrement {inc} does not divide evenly into {dividend}"));
    }
    Ok(())
}

pub fn show_calendar(vm: &mut Vm, o: &Opts) -> Result<ShowCalendar, Value> {
    Ok(match string_option(vm, o, "calendarName", &["auto", "always", "never", "critical"])?.as_deref() {
        Some("always") => ShowCalendar::Always,
        Some("never") => ShowCalendar::Never,
        Some("critical") => ShowCalendar::Critical,
        _ => ShowCalendar::Auto,
    })
}

/// GetTemporalShowTimeZoneNameOption: (mostrar, crítico).
pub fn show_time_zone(vm: &mut Vm, o: &Opts) -> Result<(bool, bool), Value> {
    Ok(match string_option(vm, o, "timeZoneName", &["auto", "never", "critical"])?.as_deref() {
        Some("never") => (false, false),
        Some("critical") => (true, true),
        _ => (true, false),
    })
}

/// GetTemporalShowOffsetOption: true = auto.
pub fn show_offset(vm: &mut Vm, o: &Opts) -> Result<bool, Value> {
    Ok(string_option(vm, o, "offset", &["auto", "never"])?.as_deref() != Some("never"))
}

/// GetDirectionOption: true = next.
pub fn direction(vm: &mut Vm, o: &Opts) -> Result<bool, Value> {
    match string_option(vm, o, "direction", &["next", "previous"])? {
        Some(s) => Ok(s == "next"),
        None => range(vm, "direction option is required"),
    }
}

/// GetTemporalFractionalSecondDigitsOption: None = auto.
pub fn fractional_digits(vm: &mut Vm, o: &Opts) -> Result<Option<u8>, Value> {
    let v = get(vm, o, "fractionalSecondDigits")?;
    if v.is_undefined() {
        return Ok(None);
    }
    let Value::Number(n) = v else {
        let s = vm.to_rust_string(&v)?;
        if s != "auto" {
            return range(vm, format!("Value {s} out of range for options property fractionalSecondDigits"));
        }
        return Ok(None);
    };
    if !n.is_finite() || !(0.0..10.0).contains(&n.floor()) {
        return range(vm, format!(
            "Value {} out of range for options property fractionalSecondDigits",
            crate::conv::number_to_string(n)
        ));
    }
    Ok(Some(n.floor() as u8))
}

/// Valor de uma opção de unidade.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnitOpt {
    Unset,
    Auto,
    U(Unit),
}

/// GetTemporalUnitValuedOption.
pub fn unit(vm: &mut Vm, o: &Opts, key: &str, required: bool) -> Result<UnitOpt, Value> {
    let v = get(vm, o, key)?;
    if v.is_undefined() {
        if required {
            return range(vm, format!("{key} option is required"));
        }
        return Ok(UnitOpt::Unset);
    }
    let s = vm.to_rust_string(&v)?;
    if s == "auto" {
        return Ok(UnitOpt::Auto);
    }
    for u in UNITS {
        let sing = u.singular();
        if s == sing || (s.len() == sing.len() + 1 && s.starts_with(sing) && s.ends_with('s')) {
            return Ok(UnitOpt::U(u));
        }
    }
    range(vm, format!("Value {s} out of range for options property {key}"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Date,
    Time,
    DateTime,
}

/// ValidateTemporalUnitValue.
pub fn validate_unit(vm: &mut Vm, v: UnitOpt, group: Group, extra: &[UnitOpt]) -> Result<(), Value> {
    if v == UnitOpt::Unset || extra.contains(&v) {
        return Ok(());
    }
    if let UnitOpt::U(u) = v {
        let ok = if u.is_date() {
            group != Group::Time
        } else {
            group != Group::Date
        };
        if ok {
            return Ok(());
        }
        return range(vm, format!("{} is not a valid unit here", u.singular()));
    }
    range(vm, "auto is not a valid unit here")
}

/// Registro de ToSecondsStringPrecisionRecord: (precisão, unidade, incremento).
/// Precisão: None = auto, Some(255) = minuto, Some(n) = n dígitos.
pub fn seconds_precision(smallest: UnitOpt, digits: Option<u8>) -> (Option<u8>, Unit, i64) {
    match smallest {
        UnitOpt::U(Unit::Minute) => (Some(PRECISION_MINUTE), Unit::Minute, 1),
        UnitOpt::U(Unit::Second) => (Some(0), Unit::Second, 1),
        UnitOpt::U(Unit::Millisecond) => (Some(3), Unit::Millisecond, 1),
        UnitOpt::U(Unit::Microsecond) => (Some(6), Unit::Microsecond, 1),
        UnitOpt::U(Unit::Nanosecond) => (Some(9), Unit::Nanosecond, 1),
        _ => match digits {
            None => (None, Unit::Nanosecond, 1),
            Some(0) => (Some(0), Unit::Second, 1),
            Some(d @ 1..=3) => (Some(d), Unit::Millisecond, 10i64.pow(3 - d as u32)),
            Some(d @ 4..=6) => (Some(d), Unit::Microsecond, 10i64.pow(6 - d as u32)),
            Some(d) => (Some(d), Unit::Nanosecond, 10i64.pow(9 - d as u32)),
        },
    }
}

pub const PRECISION_MINUTE: u8 = 255;
