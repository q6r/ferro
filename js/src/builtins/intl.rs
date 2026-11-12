//! Intl: formatação por idioma (números, datas, plurais, listas, tempo
//! relativo, comparação de textos). Dados embutidos para en, pt, es, fr,
//! de e it; outros idiomas caem no inglês.

use std::cmp::Ordering;

use super::{Realm, native_data, set_native_data};
use crate::conv::to_fixed;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{TzInfo, Vm, truthy};

// ------------------------------------------------------------ dados

pub struct LocaleData {
    pub lang: &'static str,
    decimal: &'static str,
    group: &'static str,
    min_grouping: usize,
    months: [&'static str; 12],
    months_short: [&'static str; 12],
    days: [&'static str; 7],
    days_short: [&'static str; 7],
    /// "{d}", "{M}", "{y}" (data numérica).
    numeric: &'static str,
    numeric_pad: bool,
    ymd_long: &'static str,
    ymd_medium: &'static str,
    ym_long: &'static str,
    md_long: &'static str,
    weekday_sep: &'static str,
    /// dia + mês abreviado ("Nov 11", "11 de nov.", "11 nov").
    md_short: &'static str,
    /// estilo "medium" em números (alemão: 11.11.2026)
    medium_numeric: bool,
    /// estilo "short" com ano de 2 dígitos
    short_year2: bool,
    /// separador data-hora nos estilos full/long e medium
    sep_long: &'static str,
    sep_medium: &'static str,
    hour12: bool,
    am: &'static str,
    pm: &'static str,
    datetime_sep: &'static str,
    compact_short: [&'static str; 4],
    compact_long: [(&'static str, &'static str); 4],
    currency_after: bool,
    percent_space: bool,
    list_and: &'static str,
    list_or: &'static str,
    oxford: bool,
    rel_future: &'static str,
    rel_past: &'static str,
    /// (unidade, singular, plural)
    rel_units: [(&'static str, &'static str, &'static str); 8],
    /// ontem/hoje/amanhã etc. (unidade, -1, 0, 1)
    rel_auto: [(&'static str, &'static str, &'static str, &'static str); 5],
}

const UNITS: [&str; 8] = ["second", "minute", "hour", "day", "week", "month", "quarter", "year"];

