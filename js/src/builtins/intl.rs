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

const EN_DATA: LocaleData = LocaleData {
    lang: "en",
    decimal: ".",
    group: ",",
    min_grouping: 1,
    months: ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
    months_short: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    days: ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"],
    days_short: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    numeric: "{M}/{d}/{y}",
    numeric_pad: false,
    ymd_long: "{M} {d}, {y}",
    ymd_medium: "{M} {d}, {y}",
    ym_long: "{M} {y}",
    md_long: "{M} {d}",
    weekday_sep: ", ",
    md_short: "{M} {d}",
    medium_numeric: false,
    short_year2: true,
    sep_long: " at ",
    sep_medium: ", ",
    hour12: true,
    am: "AM",
    pm: "PM",
    datetime_sep: ", ",
    compact_short: ["K", "M", "B", "T"],
    compact_long: [(" thousand", " thousand"), (" million", " million"), (" billion", " billion"), (" trillion", " trillion")],
    currency_after: false,
    percent_space: false,
    list_and: "and",
    list_or: "or",
    oxford: true,
    rel_future: "in {n} {u}",
    rel_past: "{n} {u} ago",
    rel_units: [
        ("second", "second", "seconds"),
        ("minute", "minute", "minutes"),
        ("hour", "hour", "hours"),
        ("day", "day", "days"),
        ("week", "week", "weeks"),
        ("month", "month", "months"),
        ("quarter", "quarter", "quarters"),
        ("year", "year", "years"),
    ],
    rel_auto: [
        ("day", "yesterday", "today", "tomorrow"),
        ("week", "last week", "this week", "next week"),
        ("month", "last month", "this month", "next month"),
        ("year", "last year", "this year", "next year"),
        ("quarter", "last quarter", "this quarter", "next quarter"),
    ],
};

static EN: LocaleData = EN_DATA;

/// Inglês fora dos EUA (Reino Unido, Austrália, Índia...): dia/mês e 24 h.
static EN_GB: LocaleData = LocaleData {
    numeric: "{d}/{M}/{y}",
    numeric_pad: true,
    ymd_long: "{d} {M} {y}",
    ymd_medium: "{d} {M} {y}",
    md_long: "{d} {M}",
    md_short: "{d} {M}",
    short_year2: false,
    hour12: false,
    am: "am",
    pm: "pm",
    oxford: false,
    ..EN_DATA
};

static PT: LocaleData = LocaleData {
    lang: "pt",
    decimal: ",",
    group: ".",
    min_grouping: 1,
    months: ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"],
    months_short: ["jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.", "dez."],
    days: ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"],
    days_short: ["dom.", "seg.", "ter.", "qua.", "qui.", "sex.", "sáb."],
    numeric: "{d}/{M}/{y}",
    numeric_pad: true,
    ymd_long: "{d} de {M} de {y}",
    ymd_medium: "{d} de {M} de {y}",
    ym_long: "{M} de {y}",
    md_long: "{d} de {M}",
    weekday_sep: ", ",
    md_short: "{d} de {M}",
    medium_numeric: false,
    short_year2: false,
    sep_long: " às ",
    sep_medium: ", ",
    hour12: false,
    am: "AM",
    pm: "PM",
    datetime_sep: ", ",
    compact_short: ["\u{a0}mil", "\u{a0}mi", "\u{a0}bi", "\u{a0}tri"],
    compact_long: [(" mil", " mil"), (" milhão", " milhões"), (" bilhão", " bilhões"), (" trilhão", " trilhões")],
    currency_after: false,
    percent_space: false,
    list_and: "e",
    list_or: "ou",
    oxford: false,
    rel_future: "em {n} {u}",
    rel_past: "há {n} {u}",
    rel_units: [
        ("second", "segundo", "segundos"),
        ("minute", "minuto", "minutos"),
        ("hour", "hora", "horas"),
        ("day", "dia", "dias"),
        ("week", "semana", "semanas"),
        ("month", "mês", "meses"),
        ("quarter", "trimestre", "trimestres"),
        ("year", "ano", "anos"),
    ],
    rel_auto: [
        ("day", "ontem", "hoje", "amanhã"),
        ("week", "semana passada", "esta semana", "próxima semana"),
        ("month", "mês passado", "este mês", "próximo mês"),
        ("year", "ano passado", "este ano", "próximo ano"),
        ("quarter", "último trimestre", "este trimestre", "próximo trimestre"),
    ],
};

