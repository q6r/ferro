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

static ES: LocaleData = LocaleData {
    lang: "es",
    decimal: ",",
    group: ".",
    min_grouping: 2,
    months: ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
    months_short: ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"],
    days: ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"],
    days_short: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    numeric: "{d}/{M}/{y}",
    numeric_pad: false,
    ymd_long: "{d} de {M} de {y}",
    ymd_medium: "{d} {M} {y}",
    ym_long: "{M} de {y}",
    md_long: "{d} de {M}",
    weekday_sep: ", ",
    md_short: "{d} {M}",
    medium_numeric: false,
    short_year2: true,
    sep_long: ", ",
    sep_medium: ", ",
    hour12: false,
    am: "a. m.",
    pm: "p. m.",
    datetime_sep: ", ",
    compact_short: ["\u{a0}mil", "\u{a0}M", "\u{a0}mil\u{a0}M", "\u{a0}B"],
    compact_long: [(" mil", " mil"), (" millón", " millones"), (" mil millones", " mil millones"), (" billón", " billones")],
    currency_after: true,
    percent_space: true,
    list_and: "y",
    list_or: "o",
    oxford: false,
    rel_future: "dentro de {n} {u}",
    rel_past: "hace {n} {u}",
    rel_units: [
        ("second", "segundo", "segundos"),
        ("minute", "minuto", "minutos"),
        ("hour", "hora", "horas"),
        ("day", "día", "días"),
        ("week", "semana", "semanas"),
        ("month", "mes", "meses"),
        ("quarter", "trimestre", "trimestres"),
        ("year", "año", "años"),
    ],
    rel_auto: [
        ("day", "ayer", "hoy", "mañana"),
        ("week", "la semana pasada", "esta semana", "la próxima semana"),
        ("month", "el mes pasado", "este mes", "el próximo mes"),
        ("year", "el año pasado", "este año", "el próximo año"),
        ("quarter", "el trimestre pasado", "este trimestre", "el próximo trimestre"),
    ],
};

static FR: LocaleData = LocaleData {
    lang: "fr",
    decimal: ",",
    group: "\u{202f}",
    min_grouping: 1,
    months: ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    months_short: ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    days: ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"],
    days_short: ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."],
    numeric: "{d}/{M}/{y}",
    numeric_pad: true,
    ymd_long: "{d} {M} {y}",
    ymd_medium: "{d} {M} {y}",
    ym_long: "{M} {y}",
    md_long: "{d} {M}",
    weekday_sep: " ",
    md_short: "{d} {M}",
    medium_numeric: false,
    short_year2: false,
    sep_long: " à ",
    sep_medium: ", ",
    hour12: false,
    am: "AM",
    pm: "PM",
    datetime_sep: " ",
    compact_short: ["\u{a0}k", "\u{a0}M", "\u{a0}Md", "\u{a0}Bn"],
    compact_long: [(" mille", " mille"), (" million", " millions"), (" milliard", " milliards"), (" billion", " billions")],
    currency_after: true,
    percent_space: true,
    list_and: "et",
    list_or: "ou",
    oxford: false,
    rel_future: "dans {n} {u}",
    rel_past: "il y a {n} {u}",
    rel_units: [
        ("second", "seconde", "secondes"),
        ("minute", "minute", "minutes"),
        ("hour", "heure", "heures"),
        ("day", "jour", "jours"),
        ("week", "semaine", "semaines"),
        ("month", "mois", "mois"),
        ("quarter", "trimestre", "trimestres"),
        ("year", "an", "ans"),
    ],
    rel_auto: [
        ("day", "hier", "aujourd’hui", "demain"),
        ("week", "la semaine dernière", "cette semaine", "la semaine prochaine"),
        ("month", "le mois dernier", "ce mois-ci", "le mois prochain"),
        ("year", "l’année dernière", "cette année", "l’année prochaine"),
        ("quarter", "le trimestre dernier", "ce trimestre", "le trimestre prochain"),
    ],
};

static DE: LocaleData = LocaleData {
    lang: "de",
    decimal: ",",
    group: ".",
    min_grouping: 1,
    months: ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
    months_short: ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."],
    days: ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"],
    days_short: ["So.", "Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa."],
    numeric: "{d}.{M}.{y}",
    numeric_pad: false,
    ymd_long: "{d}. {M} {y}",
    ymd_medium: "{d}.{M}.{y}",
    ym_long: "{M} {y}",
    md_long: "{d}. {M}",
    weekday_sep: ", ",
    md_short: "{d}. {M}",
    medium_numeric: true,
    short_year2: true,
    sep_long: " um ",
    sep_medium: ", ",
    hour12: false,
    am: "AM",
    pm: "PM",
    datetime_sep: ", ",
    compact_short: ["\u{a0}Tsd.", "\u{a0}Mio.", "\u{a0}Mrd.", "\u{a0}Bio."],
    compact_long: [(" Tausend", " Tausend"), (" Million", " Millionen"), (" Milliarde", " Milliarden"), (" Billion", " Billionen")],
    currency_after: true,
    percent_space: true,
    list_and: "und",
    list_or: "oder",
    oxford: false,
    rel_future: "in {n} {u}",
    rel_past: "vor {n} {u}",
    rel_units: [
        ("second", "Sekunde", "Sekunden"),
        ("minute", "Minute", "Minuten"),
        ("hour", "Stunde", "Stunden"),
        ("day", "Tag", "Tagen"),
        ("week", "Woche", "Wochen"),
        ("month", "Monat", "Monaten"),
        ("quarter", "Quartal", "Quartalen"),
        ("year", "Jahr", "Jahren"),
    ],
    rel_auto: [
        ("day", "gestern", "heute", "morgen"),
        ("week", "letzte Woche", "diese Woche", "nächste Woche"),
        ("month", "letzten Monat", "diesen Monat", "nächsten Monat"),
        ("year", "letztes Jahr", "dieses Jahr", "nächstes Jahr"),
        ("quarter", "letztes Quartal", "dieses Quartal", "nächstes Quartal"),
    ],
};

static IT: LocaleData = LocaleData {
    lang: "it",
    decimal: ",",
    group: ".",
    min_grouping: 2,
    months: ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"],
    months_short: ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"],
    days: ["domenica", "lunedì", "martedì", "mercoledì", "giovedì", "venerdì", "sabato"],
    days_short: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
    numeric: "{d}/{M}/{y}",
    numeric_pad: false,
    ymd_long: "{d} {M} {y}",
    ymd_medium: "{d} {M} {y}",
    ym_long: "{M} {y}",
    md_long: "{d} {M}",
    weekday_sep: " ",
    md_short: "{d} {M}",
    medium_numeric: false,
    short_year2: true,
    sep_long: " alle ore ",
    sep_medium: ", ",
    hour12: false,
    am: "AM",
    pm: "PM",
    datetime_sep: ", ",
    compact_short: ["", "\u{a0}Mln", "\u{a0}Mrd", "\u{a0}Bln"],
    compact_long: [(" mila", " mila"), (" milione", " milioni"), (" miliardo", " miliardi"), (" mille miliardi", " mille miliardi")],
    currency_after: true,
    percent_space: false,
    list_and: "e",
    list_or: "o",
    oxford: false,
    rel_future: "tra {n} {u}",
    rel_past: "{n} {u} fa",
    rel_units: [
        ("second", "secondo", "secondi"),
        ("minute", "minuto", "minuti"),
        ("hour", "ora", "ore"),
        ("day", "giorno", "giorni"),
        ("week", "settimana", "settimane"),
        ("month", "mese", "mesi"),
        ("quarter", "trimestre", "trimestri"),
        ("year", "anno", "anni"),
    ],
    rel_auto: [
        ("day", "ieri", "oggi", "domani"),
        ("week", "settimana scorsa", "questa settimana", "settimana prossima"),
        ("month", "mese scorso", "questo mese", "mese prossimo"),
        ("year", "anno scorso", "quest’anno", "anno prossimo"),
        ("quarter", "trimestre scorso", "questo trimestre", "trimestre prossimo"),
    ],
};

/// Idioma suportado mais próximo da etiqueta (ex.: "pt-BR" → pt).
pub fn data_for(tag: &str) -> &'static LocaleData {
    let lang = tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    match lang.as_str() {
        "pt" => &PT,
        "es" => &ES,
        "fr" => &FR,
        "de" => &DE,
        "it" => &IT,
        "en" => {
            let region = tag.split(['-', '_']).nth(1).unwrap_or("US").to_ascii_uppercase();
            if matches!(region.as_str(), "US" | "CA" | "PH" | "") { &EN } else { &EN_GB }
        }
        _ => &EN,
    }
}

/// Canonicaliza uma etiqueta ("pt-br" → "pt-BR").
fn canonical_tag(t: &str) -> Option<String> {
    let parts: Vec<&str> = t.split(['-', '_']).collect();
    if parts.is_empty() || parts[0].len() < 2 || parts[0].len() > 8 || !parts[0].chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut out = parts[0].to_ascii_lowercase();
    for p in &parts[1..] {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        out.push('-');
        if p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic()) {
            out.push_str(&p.to_ascii_uppercase());
        } else if p.len() == 4 && p.chars().all(|c| c.is_ascii_alphabetic()) {
            let mut c = p.chars();
            out.extend(c.next().map(|x| x.to_ascii_uppercase()));
            out.push_str(&c.as_str().to_ascii_lowercase());
        } else {
            out.push_str(&p.to_ascii_lowercase());
        }
    }
    Some(out)
}

/// Lista de locais pedidos (string, array ou undefined).
fn requested(vm: &mut Vm, v: &Value) -> Result<Vec<String>, Value> {
    let list: Vec<Value> = match v {
        Value::Undefined => return Ok(Vec::new()),
        Value::String(_) => vec![v.clone()],
        Value::Object(o) if matches!(&o.borrow().kind, Kind::Intl(x) if matches!(**x, IntlObj::Locale(_))) => vec![v.clone()],
        _ => {
            let o = Value::Object(vm.to_object(v)?);
            super::function::list_from_array_like(vm, &o)?
        }
    };
    let mut out = Vec::new();
    for x in list {
        let s = vm.to_rust_string(&x)?;
        match canonical_tag(&s) {
            Some(c) => out.push(c),
            None => return Err(vm.range_error(format!("Incorrect locale information provided: {s}"))),
        }
    }
    Ok(out)
}

/// O local efetivo: o primeiro pedido (todos são aceitos, os dados caem no inglês).
fn resolve_locale(vm: &mut Vm, v: &Value) -> Result<String, Value> {
    let req = requested(vm, v)?;
    Ok(req.into_iter().next().unwrap_or_else(|| vm.locale.clone()))
}

// ------------------------------------------------------------ objetos

#[derive(Clone)]
pub struct NumOpts {
    pub locale: String,
    style: String,
    currency: Option<String>,
    currency_display: String,
    unit: Option<String>,
    unit_display: String,
    min_int: usize,
    min_frac: usize,
    max_frac: usize,
    max_sig: Option<usize>,
    min_sig: Option<usize>,
    grouping: bool,
    notation: String,
    compact_display: String,
    sign_display: String,
}

#[derive(Clone, Default)]
pub struct DateOpts {
    pub locale: String,
    time_zone: Option<String>,
    weekday: Option<String>,
    era: Option<String>,
    year: Option<String>,
    month: Option<String>,
    day: Option<String>,
    hour: Option<String>,
    minute: Option<String>,
    second: Option<String>,
    frac: Option<usize>,
    tz_name: Option<String>,
    hour12: Option<bool>,
    date_style: Option<String>,
    time_style: Option<String>,
}

pub enum IntlObj {
    NumberFormat(NumOpts),
    DateTimeFormat(DateOpts),
    PluralRules { locale: String, ordinal: bool },
    RelativeTime { locale: String, numeric_auto: bool, style: String },
    ListFormat { locale: String, kind: String, style: String },
    Collator { locale: String, numeric: bool, sensitivity: String },
    Segmenter { locale: String, granularity: String },
    Locale(String),
    DisplayNames { locale: String, kind: String },
}

pub fn install(r: &Realm) {
    let intl = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.tag(&intl, "Intl");
    r.method(&intl, "getCanonicalLocales", 1, get_canonical_locales);
    r.method(&intl, "supportedValuesOf", 1, supported_values_of);
    let mk = |name: &str, f: NativeFn, len: u32| {
        let proto = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
        let c = super::native_fn(&r.function_proto, name, len, f, true);
        c.borrow_mut().props.insert(
            PropKey::str("prototype"),
            Property {
                slot: Slot::Data {
                    value: Value::Object(proto.clone()),
                    writable: false,
                },
                enumerable: false,
                configurable: false,
            },
        );
        proto.borrow_mut().props.insert(PropKey::str("constructor"), Property::hidden(Value::Object(c.clone())));
        r.tag(&proto, &format!("Intl.{name}"));
        r.method(&c, "supportedLocalesOf", 1, supported_locales_of);
        intl.borrow_mut().props.insert(PropKey::str(name), Property::hidden(Value::Object(c)));
        proto
    };
    let nf = mk("NumberFormat", number_format_ctor, 0);
    r.getter(&nf, "format", nf_format_getter);
    r.method(&nf, "formatToParts", 1, nf_format_to_parts);
    r.method(&nf, "resolvedOptions", 0, resolved_options);
    let df = mk("DateTimeFormat", date_format_ctor, 0);
    r.getter(&df, "format", df_format_getter);
    r.method(&df, "formatToParts", 1, df_format_to_parts);
    r.method(&df, "resolvedOptions", 0, resolved_options);
    let pr = mk("PluralRules", plural_ctor, 0);
    r.method(&pr, "select", 1, plural_select);
    r.method(&pr, "resolvedOptions", 0, resolved_options);
    let rt = mk("RelativeTimeFormat", relative_ctor, 0);
    r.method(&rt, "format", 2, relative_format);
    r.method(&rt, "resolvedOptions", 0, resolved_options);
    let lf = mk("ListFormat", list_ctor, 0);
    r.method(&lf, "format", 1, list_format);
    r.method(&lf, "resolvedOptions", 0, resolved_options);
    let co = mk("Collator", collator_ctor, 0);
    r.getter(&co, "compare", collator_compare_getter);
    r.method(&co, "resolvedOptions", 0, resolved_options);
    let sg = mk("Segmenter", segmenter_ctor, 0);
    r.method(&sg, "segment", 1, segment);
    r.method(&sg, "resolvedOptions", 0, resolved_options);
    let lo = mk("Locale", locale_ctor, 1);
    for (name, f) in [
        ("language", locale_language as NativeFn),
        ("region", locale_region),
        ("baseName", locale_base_name),
        ("script", locale_script),
    ] {
        r.getter(&lo, name, f);
    }
    r.method(&lo, "toString", 0, locale_to_string);
    r.method(&lo, "maximize", 0, locale_maximize);
    r.method(&lo, "minimize", 0, locale_maximize);
    let dn = mk("DisplayNames", display_names_ctor, 2);
    r.method(&dn, "of", 1, display_names_of);
    r.global_value("Intl", Value::Object(intl));
}

fn intl_obj(vm: &mut Vm, nt: &Option<ObjRef>, callee: &ObjRef, data: IntlObj) -> Result<Value, Value> {
    let def = match vm.get_obj(callee, &PropKey::str("prototype"))? {
        Value::Object(p) => p,
        _ => vm.realm.object_proto.clone(),
    };
    let proto = vm.proto_from_ctor(nt.as_ref(), &def)?;
    Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::Intl(Box::new(data))))))
}

fn with_intl<R>(vm: &mut Vm, v: &Value, f: impl FnOnce(&IntlObj) -> Option<R>) -> Result<R, Value> {
    if let Value::Object(o) = v
        && let Kind::Intl(x) = &o.borrow().kind
        && let Some(r) = f(x)
    {
        return Ok(r);
    }
    Err(vm.type_error("Method called on incompatible receiver"))
}

fn opt_str(vm: &mut Vm, opts: &Value, name: &str, allowed: &[&str]) -> Result<Option<String>, Value> {
    if !matches!(opts, Value::Object(_)) {
        return Ok(None);
    }
    let v = vm.get_str(opts, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    let s = vm.to_rust_string(&v)?;
    if !allowed.is_empty() && !allowed.contains(&s.as_str()) {
        return Err(vm.range_error(format!("Value {s} out of range for Intl options property {name}")));
    }
    Ok(Some(s))
}

fn opt_num(vm: &mut Vm, opts: &Value, name: &str, min: f64, max: f64) -> Result<Option<usize>, Value> {
    if !matches!(opts, Value::Object(_)) {
        return Ok(None);
    }
    let v = vm.get_str(opts, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    let n = vm.to_number(&v)?;
    if n.is_nan() || n < min || n > max {
        return Err(vm.range_error(format!("{name} value is out of range.")));
    }
    Ok(Some(n.floor() as usize))
}

fn opt_bool(vm: &mut Vm, opts: &Value, name: &str) -> Result<Option<bool>, Value> {
    if !matches!(opts, Value::Object(_)) {
        return Ok(None);
    }
    let v = vm.get_str(opts, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(truthy(&v)))
}

fn get_canonical_locales(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let list: Vec<Value> = requested(vm, &a.arg(0))?.into_iter().map(|s| Value::str(&s)).collect();
    Ok(vm.array_value(list))
}

fn supported_locales_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let list: Vec<Value> = requested(vm, &a.arg(0))?
        .into_iter()
        .filter(|t| matches!(t.split('-').next(), Some("en" | "pt" | "es" | "fr" | "de" | "it")))
        .map(|s| Value::str(&s))
        .collect();
    Ok(vm.array_value(list))
}

fn supported_values_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = vm.to_rust_string(&a.arg(0))?;
    let vals: &[&str] = match k.as_str() {
        "calendar" => &["gregory"],
        "currency" => &["BRL", "EUR", "GBP", "JPY", "USD"],
        "numberingSystem" => &["latn"],
        "timeZone" => &["UTC"],
        "unit" => &["byte", "day", "hour", "kilobyte", "kilometer", "meter", "minute", "percent", "second"],
        "collation" => &["default"],
        _ => return Err(vm.range_error(format!("Invalid key : {k}"))),
    };
    Ok(vm.array_value(vals.iter().map(|s| Value::str(s)).collect()))
}

fn resolved_options(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let pairs: Vec<(String, Value)> = with_intl(vm, &a.this, |x| {
        let s = |v: &str| Value::str(v);
        Some(match x {
            IntlObj::NumberFormat(o) => {
                let mut v = vec![
                    ("locale".into(), s(&o.locale)),
                    ("numberingSystem".into(), s("latn")),
                    ("style".into(), s(&o.style)),
                ];
                if let Some(c) = &o.currency {
                    v.push(("currency".into(), s(c)));
                    v.push(("currencyDisplay".into(), s(&o.currency_display)));
                }
                if let Some(u) = &o.unit {
                    v.push(("unit".into(), s(u)));
                    v.push(("unitDisplay".into(), s(&o.unit_display)));
                }
                v.push(("minimumIntegerDigits".into(), Value::Number(o.min_int as f64)));
                v.push(("minimumFractionDigits".into(), Value::Number(o.min_frac as f64)));
                v.push(("maximumFractionDigits".into(), Value::Number(o.max_frac as f64)));
                if let Some(m) = o.max_sig {
                    v.push(("minimumSignificantDigits".into(), Value::Number(o.min_sig.unwrap_or(1) as f64)));
                    v.push(("maximumSignificantDigits".into(), Value::Number(m as f64)));
                }
                v.push(("useGrouping".into(), if o.grouping { s("auto") } else { Value::Bool(false) }));
                v.push(("notation".into(), s(&o.notation)));
                if o.notation == "compact" {
                    v.push(("compactDisplay".into(), s(&o.compact_display)));
                }
                v.push(("signDisplay".into(), s(&o.sign_display)));
                v
            }
            IntlObj::DateTimeFormat(o) => {
                let mut v = vec![
                    ("locale".into(), s(&o.locale)),
                    ("calendar".into(), s("gregory")),
                    ("numberingSystem".into(), s("latn")),
                    ("timeZone".into(), s(o.time_zone.as_deref().unwrap_or("UTC"))),
                ];
                if o.hour.is_some() || o.time_style.is_some() {
                    let h12 = o.hour12.unwrap_or_else(|| data_for(&o.locale).hour12);
                    v.push(("hourCycle".into(), s(if h12 { "h12" } else { "h23" })));
                    v.push(("hour12".into(), Value::Bool(h12)));
                }
                for (k, val) in [
                    ("weekday", &o.weekday),
                    ("era", &o.era),
                    ("year", &o.year),
                    ("month", &o.month),
                    ("day", &o.day),
                    ("hour", &o.hour),
                    ("minute", &o.minute),
                    ("second", &o.second),
                    ("timeZoneName", &o.tz_name),
                    ("dateStyle", &o.date_style),
                    ("timeStyle", &o.time_style),
                ] {
                    if let Some(x) = val {
                        v.push((k.into(), s(x)));
                    }
                }
                v
            }
            IntlObj::PluralRules { locale, ordinal } => vec![
                ("locale".into(), s(locale)),
                ("type".into(), s(if *ordinal { "ordinal" } else { "cardinal" })),
                ("minimumIntegerDigits".into(), Value::Number(1.0)),
                ("minimumFractionDigits".into(), Value::Number(0.0)),
                ("maximumFractionDigits".into(), Value::Number(3.0)),
            ],
            IntlObj::RelativeTime { locale, numeric_auto, style } => vec![
                ("locale".into(), s(locale)),
                ("style".into(), s(style)),
                ("numeric".into(), s(if *numeric_auto { "auto" } else { "always" })),
                ("numberingSystem".into(), s("latn")),
            ],
            IntlObj::ListFormat { locale, kind, style } => vec![
                ("locale".into(), s(locale)),
                ("type".into(), s(kind)),
                ("style".into(), s(style)),
            ],
            IntlObj::Collator { locale, numeric, sensitivity } => vec![
                ("locale".into(), s(locale)),
                ("usage".into(), s("sort")),
                ("sensitivity".into(), s(sensitivity)),
                ("ignorePunctuation".into(), Value::Bool(false)),
                ("collation".into(), s("default")),
                ("numeric".into(), Value::Bool(*numeric)),
                ("caseFirst".into(), s("false")),
            ],
            IntlObj::Segmenter { locale, granularity } => vec![
                ("locale".into(), s(locale)),
                ("granularity".into(), s(granularity)),
            ],
            IntlObj::Locale(t) => vec![("locale".into(), s(t))],
            IntlObj::DisplayNames { locale, kind } => vec![("locale".into(), s(locale)), ("type".into(), s(kind))],
        })
    })?;
    let o = vm.new_object();
    for (k, v) in pairs {
        vm.define_data(&o, PropKey::Str(JsStr::new(&k)), v);
    }
    Ok(Value::Object(o))
}

// ---------------------------------------------------------------- números

fn currency_symbol(code: &str, lang: &str, display: &str) -> String {
    if display == "code" {
        return code.to_string();
    }
    let narrow = display == "narrowSymbol";
    match code {
        "USD" => if lang == "en" || narrow { "$" } else { "US$" }.into(),
        "BRL" => "R$".into(),
        "EUR" => "€".into(),
        "GBP" => "£".into(),
        "JPY" => if lang == "en" || narrow { "¥" } else { "JP¥" }.into(),
        "CNY" => "CN¥".into(),
        "INR" => "₹".into(),
        "KRW" => "₩".into(),
        "MXN" => if narrow { "$" } else { "MX$" }.into(),
        "CAD" => if narrow { "$" } else { "CA$" }.into(),
        "AUD" => if narrow { "$" } else { "A$" }.into(),
        _ => code.into(),
    }
}

fn currency_digits(code: &str) -> usize {
    match code {
        "JPY" | "KRW" | "CLP" | "VND" | "ISK" => 0,
        _ => 2,
    }
}

fn number_format_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let locale = resolve_locale(vm, &a.arg(0))?;
    let opts = make_num_opts(vm, locale, &a.arg(1))?;
    intl_obj(vm, &a.new_target, &a.callee, IntlObj::NumberFormat(opts))
}

pub fn make_num_opts(vm: &mut Vm, locale: String, o: &Value) -> Result<NumOpts, Value> {
    let style = opt_str(vm, o, "style", &["decimal", "percent", "currency", "unit"])?.unwrap_or_else(|| "decimal".into());
    let currency = opt_str(vm, o, "currency", &[])?.map(|c| c.to_ascii_uppercase());
    if style == "currency" && currency.is_none() {
        return Err(vm.type_error("Currency code is required with currency style."));
    }
    let currency_display = opt_str(vm, o, "currencyDisplay", &["symbol", "narrowSymbol", "code", "name"])?.unwrap_or_else(|| "symbol".into());
    let unit = opt_str(vm, o, "unit", &[])?;
    if style == "unit" && unit.is_none() {
        return Err(vm.type_error("Unit is required with unit style."));
    }
    let unit_display = opt_str(vm, o, "unitDisplay", &["short", "narrow", "long"])?.unwrap_or_else(|| "short".into());
    let notation = opt_str(vm, o, "notation", &["standard", "scientific", "engineering", "compact"])?.unwrap_or_else(|| "standard".into());
    let compact_display = opt_str(vm, o, "compactDisplay", &["short", "long"])?.unwrap_or_else(|| "short".into());
    let sign_display = opt_str(vm, o, "signDisplay", &["auto", "never", "always", "exceptZero", "negative"])?.unwrap_or_else(|| "auto".into());
    let min_int = opt_num(vm, o, "minimumIntegerDigits", 1.0, 21.0)?.unwrap_or(1);
    let (def_min, def_max) = match style.as_str() {
        "currency" => {
            let d = currency_digits(currency.as_deref().unwrap_or("USD"));
            (d, d)
        }
        "percent" => (0, 0),
        _ => (0, if notation == "compact" { 0 } else { 3 }),
    };
    let mnfd = opt_num(vm, o, "minimumFractionDigits", 0.0, 100.0)?;
    let mxfd = opt_num(vm, o, "maximumFractionDigits", 0.0, 100.0)?;
    let min_frac = mnfd.unwrap_or_else(|| def_min.min(mxfd.unwrap_or(def_min)));
    let max_frac = mxfd.unwrap_or_else(|| def_max.max(min_frac));
    if min_frac > max_frac {
        return Err(vm.range_error("maximumFractionDigits value is out of range."));
    }
    let max_sig = opt_num(vm, o, "maximumSignificantDigits", 1.0, 21.0)?;
    let min_sig = opt_num(vm, o, "minimumSignificantDigits", 1.0, 21.0)?;
    let grouping = match opt_str(vm, o, "useGrouping", &[]) {
        Ok(Some(s)) => s != "false",
        _ => true,
    };
    Ok(NumOpts {
        locale,
        style,
        currency,
        currency_display,
        unit,
        unit_display,
        min_int,
        min_frac,
        max_frac,
        max_sig,
        min_sig,
        grouping,
        notation,
        compact_display,
        sign_display,
    })
}

/// Arredonda "meio para longe do zero" com `frac` casas e devolve (inteiro, fração).
fn round_digits(x: f64, frac: usize) -> (String, String) {
    let s = to_fixed(x.abs(), frac);
    match s.split_once('.') {
        Some((i, f)) => (i.to_string(), f.to_string()),
        None => (s, String::new()),
    }
}

fn round_sig(x: f64, sig: usize) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let e = x.abs().log10().floor() as i32;
    let scale = 10f64.powi(sig as i32 - 1 - e);
    (x * scale).round() / scale
}

fn group_int(int: &str, sep: &str, min_grouping: usize) -> String {
    if int.len() < 4 || int.len() < 3 + min_grouping {
        return int.to_string();
    }
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push_str(sep);
        }
        out.push(ch);
    }
    out
}

fn plural_cardinal(lang: &str, n: f64) -> &'static str {
    let i = n.abs().trunc();
    let int = n.fract() == 0.0;
    match lang {
        "pt" | "fr" => {
            if i == 0.0 || i == 1.0 {
                "one"
            } else {
                "other"
            }
        }
        _ => {
            if n.abs() == 1.0 && int {
                "one"
            } else {
                "other"
            }
        }
    }
}

fn plural_ordinal(lang: &str, n: f64) -> &'static str {
    if lang != "en" {
        return "other";
    }
    let n = n.abs();
    let (m10, m100) = (n % 10.0, n % 100.0);
    if m10 == 1.0 && m100 != 11.0 {
        "one"
    } else if m10 == 2.0 && m100 != 12.0 {
        "two"
    } else if m10 == 3.0 && m100 != 13.0 {
        "few"
    } else {
        "other"
    }
}

/// Formata um número em partes (tipo, texto).
pub fn format_number_parts(o: &NumOpts, x: f64) -> Vec<(&'static str, String)> {
    let d = data_for(&o.locale);
    let mut parts: Vec<(&'static str, String)> = Vec::new();
    let negative = x < 0.0 || (x == 0.0 && x.is_sign_negative());
    let mut v = if o.style == "percent" { x * 100.0 } else { x };
    if v.is_nan() {
        parts.push(("nan", "NaN".into()));
        return parts;
    }
    // notação compacta
    let mut compact: Option<String> = None;
    if o.notation == "compact" && v.is_finite() && v.abs() >= 1000.0 {
        let mag = ((v.abs().log10() / 3.0).floor() as usize).clamp(1, 4);
        let scaled = v / 10f64.powi(3 * mag as i32);
        let digits_int = scaled.abs().trunc().to_string().len();
        let rounded = if o.max_sig.is_none() && o.max_frac == 0 {
            if digits_int == 1 { (scaled * 10.0).round() / 10.0 } else { scaled.round() }
        } else {
            scaled
        };
        // 999.95K vira 1M
        if rounded.abs() >= 1000.0 && mag < 4 {
            v = rounded / 1000.0;
            compact = Some(compact_suffix(d, o, mag + 1, v));
        } else {
            v = rounded;
            compact = Some(compact_suffix(d, o, mag, v));
        }
    }
    let (int, frac) = if !v.is_finite() {
        ("∞".to_string(), String::new())
    } else if let Some(ms) = o.max_sig {
        let r = round_sig(v.abs(), ms);
        let s = crate::conv::number_to_string(r);
        let (i, f) = match s.split_once('.') {
            Some((i, f)) => (i.to_string(), f.to_string()),
            None => (s, String::new()),
        };
        let min_sig = o.min_sig.unwrap_or(1);
        let mut f = f;
        let have = i.trim_start_matches('0').len() + f.len();
        if have < min_sig {
            f.push_str(&"0".repeat(min_sig - have));
        }
        (i, f)
    } else {
        let max = if compact.is_some() && o.max_frac == 0 { 1 } else { o.max_frac };
        let (i, mut f) = round_digits(v, max);
        while f.len() > o.min_frac && f.ends_with('0') {
            f.pop();
        }
        (i, f)
    };
    let int = if int.len() < o.min_int { format!("{}{int}", "0".repeat(o.min_int - int.len())) } else { int };
    let is_zero = int.chars().all(|c| c == '0') && frac.chars().all(|c| c == '0');
    let sign = match o.sign_display.as_str() {
        "never" => "",
        "always" => {
            if negative {
                "-"
            } else {
                "+"
            }
        }
        "exceptZero" => {
            if is_zero {
                ""
            } else if negative {
                "-"
            } else {
                "+"
            }
        }
        "negative" => {
            if negative && !is_zero {
                "-"
            } else {
                ""
            }
        }
        _ => {
            if negative && !(is_zero && x == 0.0 && o.sign_display == "auto" && !x.is_sign_negative()) {
                "-"
            } else {
                ""
            }
        }
    };
    let currency = o.currency.as_deref().map(|c| currency_symbol(c, d.lang, &o.currency_display));
    let cur_before = o.style == "currency" && !d.currency_after;
    if !sign.is_empty() {
        parts.push((if sign == "-" { "minusSign" } else { "plusSign" }, sign.into()));
    }
    if cur_before && let Some(c) = &currency {
        parts.push(("currency", c.clone()));
        if d.lang != "en" || o.currency_display == "code" {
            parts.push(("literal", "\u{a0}".into()));
        }
    }
    let grouped = if o.grouping && int != "∞" { group_int(&int, d.group, d.min_grouping) } else { int };
    for (k, chunk) in grouped.split(d.group).enumerate() {
        if k > 0 {
            parts.push(("group", d.group.into()));
        }
        parts.push(("integer", chunk.into()));
    }
    if !frac.is_empty() {
        parts.push(("decimal", d.decimal.into()));
        parts.push(("fraction", frac));
    }
    if let Some(c) = compact {
        if !c.is_empty() {
            let (lit, word) = match c.strip_prefix([' ', '\u{a0}']) {
                Some(w) => (Some(c[..c.len() - w.len()].to_string()), w.to_string()),
                None => (None, c.clone()),
            };
            if let Some(l) = lit {
                parts.push(("literal", l));
            }
            parts.push(("compact", word));
        }
    }
    if o.style == "percent" {
        if d.percent_space {
            parts.push(("literal", "\u{a0}".into()));
        }
        parts.push(("percentSign", "%".into()));
    }
    if o.style == "currency"
        && !cur_before
        && let Some(c) = &currency
    {
        parts.push(("literal", "\u{a0}".into()));
        parts.push(("currency", c.clone()));
    }
    if o.style == "unit"
        && let Some(u) = &o.unit
    {
        let (text, space) = unit_text(u, &o.unit_display, v);
        if space {
            parts.push(("literal", " ".into()));
        }
        parts.push(("unit", text));
    }
    parts
}

fn compact_suffix(d: &LocaleData, o: &NumOpts, mag: usize, v: f64) -> String {
    let i = mag.clamp(1, 4) - 1;
    if o.compact_display == "long" {
        let (one, other) = d.compact_long[i];
        if plural_cardinal(d.lang, v) == "one" { one.into() } else { other.into() }
    } else {
        d.compact_short[i].into()
    }
}

fn unit_text(unit: &str, display: &str, v: f64) -> (String, bool) {
    let long = display == "long";
    let one = v.abs() == 1.0;
    let (short, long_one, long_other, space): (&str, &str, &str, bool) = match unit {
        "percent" => ("%", "percent", "percent", false),
        "kilometer" => ("km", "kilometer", "kilometers", true),
        "meter" => ("m", "meter", "meters", true),
        "centimeter" => ("cm", "centimeter", "centimeters", true),
        "kilogram" => ("kg", "kilogram", "kilograms", true),
        "gram" => ("g", "gram", "grams", true),
        "byte" => ("byte", "byte", "bytes", true),
        "kilobyte" => ("kB", "kilobyte", "kilobytes", true),
        "megabyte" => ("MB", "megabyte", "megabytes", true),
        "gigabyte" => ("GB", "gigabyte", "gigabytes", true),
        "second" => ("sec", "second", "seconds", true),
        "minute" => ("min", "minute", "minutes", true),
        "hour" => ("hr", "hour", "hours", true),
        "day" => (if one { "day" } else { "days" }, "day", "days", true),
        "kilometer-per-hour" => ("km/h", "kilometer per hour", "kilometers per hour", true),
        "celsius" => ("°C", "degree Celsius", "degrees Celsius", false),
        "liter" => ("L", "liter", "liters", true),
        other => (other, other, other, true),
    };
    if long {
        ((if one { long_one } else { long_other }).into(), true)
    } else {
        (short.into(), space)
    }
}

pub fn format_number(o: &NumOpts, x: f64) -> String {
    format_number_parts(o, x).into_iter().map(|(_, s)| s).collect()
}

fn nf_value(vm: &mut Vm, v: &Value) -> Result<f64, Value> {
    match vm.to_numeric(v)? {
        Value::BigInt(b) => Ok(b.to_f64()),
        n => vm.to_number(&n),
    }
}

fn nf_format_getter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    with_intl(vm, &a.this, |x| matches!(x, IntlObj::NumberFormat(_)).then_some(()))?;
    let f = vm.realm.native("", 1, nf_bound_format);
    set_native_data(&f, a.this.clone());
    Ok(Value::Object(f))
}

fn nf_bound_format(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let this = native_data(&a.callee);
    let o = with_intl(vm, &this, |x| match x {
        IntlObj::NumberFormat(o) => Some(o.clone()),
        _ => None,
    })?;
    let n = nf_value(vm, &a.arg(0))?;
    Ok(Value::str(&format_number(&o, n)))
}

fn parts_to_array(vm: &mut Vm, parts: Vec<(&'static str, String)>) -> Value {
    let mut out = Vec::new();
    for (t, v) in parts {
        let p = vm.new_object();
        vm.define_data(&p, PropKey::str("type"), Value::str(t));
        vm.define_data(&p, PropKey::str("value"), Value::str(&v));
        out.push(Value::Object(p));
    }
    vm.array_value(out)
}

fn nf_format_to_parts(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = with_intl(vm, &a.this, |x| match x {
        IntlObj::NumberFormat(o) => Some(o.clone()),
        _ => None,
    })?;
    let n = nf_value(vm, &a.arg(0))?;
    let parts = format_number_parts(&o, n);
    Ok(parts_to_array(vm, parts))
}

/// Number.prototype.toLocaleString.
pub fn number_to_locale_string(vm: &mut Vm, n: f64, locales: &Value, options: &Value) -> Result<String, Value> {
    let locale = resolve_locale(vm, locales)?;
    let o = make_num_opts(vm, locale, options)?;
    Ok(format_number(&o, n))
}

// ----------------------------------------------------------------- datas

fn date_format_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let locale = resolve_locale(vm, &a.arg(0))?;
    let opts = make_date_opts(vm, locale, &a.arg(1), "any", "date")?;
    intl_obj(vm, &a.new_target, &a.callee, IntlObj::DateTimeFormat(opts))
}

/// `required`: "date", "time" ou "any"; `defaults`: o que mostrar se nada foi pedido.
pub fn make_date_opts(vm: &mut Vm, locale: String, o: &Value, required: &str, defaults: &str) -> Result<DateOpts, Value> {
    let mut d = DateOpts {
        locale,
        ..Default::default()
    };
    let tz = opt_str(vm, o, "timeZone", &[])?;
    d.time_zone = match tz {
        Some(z) => {
            let canon = if z.eq_ignore_ascii_case("utc") || z.eq_ignore_ascii_case("etc/utc") || z.eq_ignore_ascii_case("gmt") { "UTC".to_string() } else { z };
            if (vm.tz)(Some(&canon), 0.0).is_none() {
                return Err(vm.range_error(format!("Invalid time zone specified: {canon}")));
            }
            Some(canon)
        }
        None => None,
    };
    let text = ["narrow", "short", "long"];
    let num = ["numeric", "2-digit"];
    d.weekday = opt_str(vm, o, "weekday", &text)?;
    d.era = opt_str(vm, o, "era", &text)?;
    d.year = opt_str(vm, o, "year", &num)?;
    d.month = opt_str(vm, o, "month", &["numeric", "2-digit", "narrow", "short", "long"])?;
    d.day = opt_str(vm, o, "day", &num)?;
    d.hour = opt_str(vm, o, "hour", &num)?;
    d.minute = opt_str(vm, o, "minute", &num)?;
    d.second = opt_str(vm, o, "second", &num)?;
    d.frac = opt_num(vm, o, "fractionalSecondDigits", 1.0, 3.0)?;
    d.tz_name = opt_str(vm, o, "timeZoneName", &["short", "long", "shortOffset", "longOffset", "shortGeneric", "longGeneric"])?;
    d.hour12 = opt_bool(vm, o, "hour12")?;
    if d.hour12.is_none()
        && let Some(hc) = opt_str(vm, o, "hourCycle", &["h11", "h12", "h23", "h24"])?
    {
        d.hour12 = Some(hc == "h11" || hc == "h12");
    }
    let styles = ["full", "long", "medium", "short"];
    d.date_style = opt_str(vm, o, "dateStyle", &styles)?;
    d.time_style = opt_str(vm, o, "timeStyle", &styles)?;
    let has_date = d.weekday.is_some() || d.year.is_some() || d.month.is_some() || d.day.is_some();
    let has_time = d.hour.is_some() || d.minute.is_some() || d.second.is_some() || d.frac.is_some();
    if (d.date_style.is_some() || d.time_style.is_some()) && (has_date || has_time) {
        return Err(vm.type_error("Can't set option dateStyle or timeStyle when other date/time options are set"));
    }
    let need_defaults = d.date_style.is_none()
        && d.time_style.is_none()
        && match required {
            "date" => !has_date,
            "time" => !has_time,
            _ => !has_date && !has_time,
        };
    if need_defaults {
        if defaults == "date" || defaults == "all" {
            d.year = Some("numeric".into());
            d.month = Some("numeric".into());
            d.day = Some("numeric".into());
        }
        if defaults == "time" || defaults == "all" {
            d.hour = Some("numeric".into());
            d.minute = Some("numeric".into());
            d.second = Some("numeric".into());
        }
    }
    Ok(d)
}

/// Componentes de data/hora (já no fuso pedido).
struct Fields {
    year: i64,
    month: usize,
    day: i64,
    weekday: usize,
    hour: i64,
    minute: i64,
    second: i64,
    ms: i64,
}

fn pad2(n: i64) -> String {
    format!("{n:02}")
}

pub fn format_date_parts(vm: &mut Vm, o: &DateOpts, t: f64) -> Result<Vec<(&'static str, String)>, Value> {
    if t.is_nan() {
        return Err(vm.range_error("Invalid time value"));
    }
    let tz: TzInfo = (vm.tz)(o.time_zone.as_deref(), t).unwrap_or(TzInfo {
        offset_min: 0.0,
        short: "UTC".into(),
        long: "Coordinated Universal Time".into(),
    });
    let local = t + tz.offset_min * 60_000.0;
    let p = super::date::parts_of(local);
    let f = Fields {
        year: p.0,
        month: p.1 as usize,
        day: p.2,
        weekday: p.3,
        hour: p.4,
        minute: p.5,
        second: p.6,
        ms: p.7,
    };
    let d = data_for(&o.locale);
    let mut out: Vec<(&'static str, String)> = Vec::new();
    // estilos prontos
    let (mut weekday, mut year, mut month, mut day) = (o.weekday.clone(), o.year.clone(), o.month.clone(), o.day.clone());
    let (mut hour, mut minute, mut second) = (o.hour.clone(), o.minute.clone(), o.second.clone());
    let mut tz_name = o.tz_name.clone();
    if let Some(ds) = &o.date_style {
        match ds.as_str() {
            "full" => {
                weekday = Some("long".into());
                year = Some("numeric".into());
                month = Some("long".into());
                day = Some("numeric".into());
            }
            "long" => {
                year = Some("numeric".into());
                month = Some("long".into());
                day = Some("numeric".into());
            }
            "medium" => {
                year = Some("numeric".into());
                month = Some(if d.medium_numeric { "2-digit" } else { "short" }.into());
                day = Some(if d.medium_numeric { "2-digit" } else { "numeric" }.into());
            }
            _ => {
                year = Some(if d.short_year2 { "2-digit" } else { "numeric" }.into());
                month = Some("numeric".into());
                day = Some("numeric".into());
            }
        }
    }
    if let Some(ts) = &o.time_style {
        hour = Some("numeric".into());
        minute = Some("2-digit".into());
        if ts != "short" {
            second = Some("2-digit".into());
        }
        if ts == "full" || ts == "long" {
            tz_name = Some(if ts == "full" { "long" } else { "short" }.into());
        }
    }
    // parte da data
    let has_date = year.is_some() || month.is_some() || day.is_some() || weekday.is_some();
    if has_date {
        let textual = matches!(month.as_deref(), Some("long" | "short" | "narrow"));
        let month_text = match month.as_deref() {
            Some("long") => d.months[f.month].to_string(),
            Some("short") => d.months_short[f.month].to_string(),
            Some("narrow") => d.months[f.month].chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(),
            Some("2-digit") => pad2(f.month as i64 + 1),
            Some(_) => {
                if d.numeric_pad && day.is_some() {
                    pad2(f.month as i64 + 1)
                } else {
                    (f.month + 1).to_string()
                }
            }
            None => String::new(),
        };
        let day_text = match day.as_deref() {
            Some("2-digit") => pad2(f.day),
            Some(_) => {
                if d.numeric_pad && !textual && month.is_some() {
                    pad2(f.day)
                } else {
                    f.day.to_string()
                }
            }
            None => String::new(),
        };
        let year_text = match year.as_deref() {
            Some("2-digit") => pad2(f.year.rem_euclid(100)),
            Some(_) => f.year.to_string(),
            None => String::new(),
        };
        let pattern: &str = if textual {
            match (day.is_some(), year.is_some()) {
                (true, true) => {
                    if month.as_deref() == Some("short") {
                        d.ymd_medium
                    } else {
                        d.ymd_long
                    }
                }
                (false, true) => d.ym_long,
                (true, false) => {
                    if month.as_deref() == Some("short") {
                        d.md_short
                    } else {
                        d.md_long
                    }
                }
                (false, false) => "{M}",
            }
        } else {
            match (day.is_some(), month.is_some(), year.is_some()) {
                (true, true, true) => d.numeric,
                (false, true, true) => {
                    if d.lang == "en" {
                        "{M}/{y}"
                    } else {
                        "{M}/{y}"
                    }
                }
                (true, true, false) => {
                    if d.numeric.starts_with("{M}") {
                        "{M}/{d}"
                    } else if d.lang == "de" {
                        "{d}.{M}."
                    } else {
                        "{d}/{M}"
                    }
                }
                (true, false, false) => "{d}",
                (false, true, false) => "{M}",
                (false, false, true) => "{y}",
                (true, false, true) => "{d} {y}",
                (false, false, false) => "",
            }
        };
        if let Some(w) = &weekday {
            let wt = match w.as_str() {
                "long" => d.days[f.weekday].to_string(),
                "short" => d.days_short[f.weekday].to_string(),
                _ => d.days[f.weekday].chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(),
            };
            out.push(("weekday", wt));
            if !pattern.is_empty() {
                out.push(("literal", d.weekday_sep.into()));
            }
        }
        // expande o padrão em partes
        let mut rest = pattern;
        while !rest.is_empty() {
            if let Some(stripped) = rest.strip_prefix("{d}") {
                out.push(("day", day_text.clone()));
                rest = stripped;
            } else if let Some(stripped) = rest.strip_prefix("{M}") {
                out.push(("month", month_text.clone()));
                rest = stripped;
            } else if let Some(stripped) = rest.strip_prefix("{y}") {
                out.push(("year", year_text.clone()));
                rest = stripped;
            } else {
                let next = rest.find('{').unwrap_or(rest.len());
                out.push(("literal", rest[..next].to_string()));
                rest = &rest[next..];
            }
        }
    }
    // parte da hora
    let has_time = hour.is_some() || minute.is_some() || second.is_some();
    if has_time {
        if has_date {
            let sep = match o.date_style.as_deref() {
                Some("full" | "long") => d.sep_long,
                Some("medium") => d.sep_medium,
                _ => d.datetime_sep,
            };
            out.push(("literal", sep.into()));
        }
        let h12 = o.hour12.unwrap_or(d.hour12);
        if let Some(h) = &hour {
            let hv = if h12 {
                let x = f.hour % 12;
                if x == 0 { 12 } else { x }
            } else {
                f.hour
            };
            let text = if h == "2-digit" || (!h12 && (minute.is_some() || d.lang != "en")) { pad2(hv) } else { hv.to_string() };
            out.push(("hour", text));
        }
        if let Some(_m) = &minute {
            if hour.is_some() {
                out.push(("literal", ":".into()));
            }
            out.push(("minute", if hour.is_some() || _m == "2-digit" { pad2(f.minute) } else { f.minute.to_string() }));
        }
        if let Some(_s) = &second {
            if hour.is_some() || minute.is_some() {
                out.push(("literal", ":".into()));
            }
            out.push(("second", if minute.is_some() || _s == "2-digit" { pad2(f.second) } else { f.second.to_string() }));
        }
        if let Some(n) = o.frac {
            out.push(("literal", ".".into()));
            out.push(("fractionalSecond", format!("{:03}", f.ms)[..n].to_string()));
        }
        if h12 && hour.is_some() {
            out.push(("literal", " ".into()));
            out.push(("dayPeriod", if f.hour < 12 { d.am } else { d.pm }.into()));
        }
    }
    if let Some(n) = &tz_name {
        out.push(("literal", " ".into()));
        let text = match n.as_str() {
            "long" | "longGeneric" => tz.long.clone(),
            "shortOffset" | "longOffset" => gmt_offset(tz.offset_min, n == "longOffset"),
            _ => {
                if tz.short.starts_with('+') || tz.short.starts_with('-') {
                    gmt_offset(tz.offset_min, false)
                } else {
                    tz.short.clone()
                }
            }
        };
        out.push(("timeZoneName", text));
    }
    Ok(out)
}

fn gmt_offset(min: f64, long: bool) -> String {
    if min == 0.0 {
        return "GMT".into();
    }
    let sign = if min < 0.0 { '-' } else { '+' };
    let m = min.abs() as i64;
    let (h, mm) = (m / 60, m % 60);
    if long {
        format!("GMT{sign}{h:02}:{mm:02}")
    } else if mm == 0 {
        format!("GMT{sign}{h}")
    } else {
        format!("GMT{sign}{h}:{mm:02}")
    }
}

pub fn format_date(vm: &mut Vm, o: &DateOpts, t: f64) -> Result<String, Value> {
    Ok(format_date_parts(vm, o, t)?.into_iter().map(|(_, s)| s).collect())
}

fn date_arg(vm: &mut Vm, v: &Value) -> Result<f64, Value> {
    if v.is_undefined() {
        return Ok(vm.epoch_now_ms());
    }
    if let Value::Object(o) = v
        && let Kind::Date(t) = o.borrow().kind
    {
        return Ok(t);
    }
    let n = vm.to_number(v)?;
    Ok(super::date::time_clip(n))
}

fn df_format_getter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    with_intl(vm, &a.this, |x| matches!(x, IntlObj::DateTimeFormat(_)).then_some(()))?;
    let f = vm.realm.native("", 1, df_bound_format);
    set_native_data(&f, a.this.clone());
    Ok(Value::Object(f))
}

fn df_bound_format(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let this = native_data(&a.callee);
    let o = with_intl(vm, &this, |x| match x {
        IntlObj::DateTimeFormat(o) => Some(o.clone()),
        _ => None,
    })?;
    let t = date_arg(vm, &a.arg(0))?;
    Ok(Value::str(&format_date(vm, &o, t)?))
}

fn df_format_to_parts(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = with_intl(vm, &a.this, |x| match x {
        IntlObj::DateTimeFormat(o) => Some(o.clone()),
        _ => None,
    })?;
    let t = date_arg(vm, &a.arg(0))?;
    let mut parts = format_date_parts(vm, &o, t)?;
    // como o V8: em formatToParts o espaço antes de AM/PM é o estreito (U+202F)
    for i in 1..parts.len() {
        if parts[i].0 == "dayPeriod" && parts[i - 1].0 == "literal" && parts[i - 1].1 == " " {
            parts[i - 1].1 = "\u{202f}".into();
        }
    }
    Ok(parts_to_array(vm, parts))
}

/// Date.prototype.toLocale{,Date,Time}String.
pub fn date_to_locale(vm: &mut Vm, t: f64, locales: &Value, options: &Value, required: &str, defaults: &str) -> Result<String, Value> {
    if t.is_nan() {
        return Ok("Invalid Date".into());
    }
    let locale = resolve_locale(vm, locales)?;
    let o = make_date_opts(vm, locale, options, required, defaults)?;
    format_date(vm, &o, t)
}

// ------------------------------------------------- plurais, tempo, listas

fn plural_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if a.new_target.is_none() {
        return Err(vm.type_error("Constructor Intl.PluralRules requires 'new'"));
    }
    let locale = resolve_locale(vm, &a.arg(0))?;
    let ordinal = opt_str(vm, &a.arg(1), "type", &["cardinal", "ordinal"])?.as_deref() == Some("ordinal");
    intl_obj(vm, &a.new_target, &a.callee, IntlObj::PluralRules { locale, ordinal })
}

fn plural_select(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (locale, ordinal) = with_intl(vm, &a.this, |x| match x {
        IntlObj::PluralRules { locale, ordinal } => Some((locale.clone(), *ordinal)),
        _ => None,
    })?;
    let n = vm.to_number(&a.arg(0))?;
    let lang = data_for(&locale).lang;
    Ok(Value::str(if ordinal { plural_ordinal(lang, n) } else { plural_cardinal(lang, n) }))
}

fn relative_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if a.new_target.is_none() {
        return Err(vm.type_error("Constructor Intl.RelativeTimeFormat requires 'new'"));
    }
    let locale = resolve_locale(vm, &a.arg(0))?;
    let numeric_auto = opt_str(vm, &a.arg(1), "numeric", &["always", "auto"])?.as_deref() == Some("auto");
    let style = opt_str(vm, &a.arg(1), "style", &["long", "short", "narrow"])?.unwrap_or_else(|| "long".into());
    intl_obj(vm, &a.new_target, &a.callee, IntlObj::RelativeTime { locale, numeric_auto, style })
}

fn relative_format(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (locale, auto) = with_intl(vm, &a.this, |x| match x {
        IntlObj::RelativeTime { locale, numeric_auto, .. } => Some((locale.clone(), *numeric_auto)),
        _ => None,
    })?;
    let v = vm.to_number(&a.arg(0))?;
    if !v.is_finite() {
        return Err(vm.range_error("Invalid time value"));
    }
    let unit_raw = vm.to_rust_string(&a.arg(1))?;
    let unit = unit_raw.strip_suffix('s').unwrap_or(&unit_raw).to_string();
    if !UNITS.contains(&unit.as_str()) {
        return Err(vm.range_error(format!("Invalid unit argument for format() '{unit_raw}'")));
    }
    let d = data_for(&locale);
    if auto {
        if v == 0.0 && unit == "second" {
            return Ok(Value::str(match d.lang {
                "pt" => "agora",
                "es" => "ahora",
                "fr" => "maintenant",
                "de" => "jetzt",
                "it" => "ora",
                _ => "now",
            }));
        }
        if let Some((_, prev, this, next)) = d.rel_auto.iter().find(|r| r.0 == unit)
            && (v == -1.0 || v == 0.0 || v == 1.0)
        {
            return Ok(Value::str(if v < 0.0 {
                prev
            } else if v == 0.0 {
                this
            } else {
                next
            }));
        }
    }
    let n = v.abs();
    let (_, one, other) = d.rel_units.iter().find(|r| r.0 == unit).copied().unwrap_or(("", "", ""));
    let word = if plural_cardinal(d.lang, n) == "one" && n == 1.0 { one } else { other };
    let word = if d.lang == "de" && v > 0.0 { word.trim_end_matches('n').replace("Tage", "Tagen") } else { word.to_string() };
    let num_opts = NumOpts {
        locale: locale.clone(),
        style: "decimal".into(),
        currency: None,
        currency_display: "symbol".into(),
        unit: None,
        unit_display: "short".into(),
        min_int: 1,
        min_frac: 0,
        max_frac: 3,
        max_sig: None,
        min_sig: None,
        grouping: true,
        notation: "standard".into(),
        compact_display: "short".into(),
        sign_display: "never".into(),
    };
    let ns = format_number(&num_opts, n);
    let pattern = if v < 0.0 || (v == 0.0 && v.is_sign_negative()) { d.rel_past } else { d.rel_future };
    Ok(Value::str(&pattern.replace("{n}", &ns).replace("{u}", &word)))
}

fn list_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if a.new_target.is_none() {
        return Err(vm.type_error("Constructor Intl.ListFormat requires 'new'"));
    }
    let locale = resolve_locale(vm, &a.arg(0))?;
    let kind = opt_str(vm, &a.arg(1), "type", &["conjunction", "disjunction", "unit"])?.unwrap_or_else(|| "conjunction".into());
    let style = opt_str(vm, &a.arg(1), "style", &["long", "short", "narrow"])?.unwrap_or_else(|| "long".into());
    intl_obj(vm, &a.new_target, &a.callee, IntlObj::ListFormat { locale, kind, style })
}

fn list_format(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (locale, kind, style) = with_intl(vm, &a.this, |x| match x {
        IntlObj::ListFormat { locale, kind, style } => Some((locale.clone(), kind.clone(), style.clone())),
        _ => None,
    })?;
    let items: Vec<String> = vm.iterate_to_vec(&a.arg(0))?.iter().map(|v| match v {
        Value::String(s) => Ok(s.to_rust()),
        _ => Err(()),
    }).collect::<Result<_, _>>().map_err(|_| vm.type_error("Iterable yielded a non-String value"))?;
    let d = data_for(&locale);
    let word = match kind.as_str() {
        "disjunction" => d.list_or,
        "unit" => "",
        _ => {
            if d.lang == "en" && style != "long" {
                "&"
            } else {
                d.list_and
            }
        }
    };
    let out = match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        n => {
            if word.is_empty() {
                items.join(if style == "narrow" { " " } else { ", " })
            } else {
                let head = items[..n - 1].join(", ");
                let comma = if d.oxford && n > 2 { "," } else { "" };
                format!("{head}{comma} {word} {}", items[n - 1])
            }
        }
    };
    Ok(Value::str(&out))
}

// ----------------------------------------------------------- comparação

/// Letra base (sem acento) para comparação.
fn base_char(c: char) -> char {
    let s = match c {
        'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
        'À'..='Å' | 'Ā' | 'Ă' | 'Ą' => 'A',
        'ç' | 'ć' | 'č' => 'c',
        'Ç' | 'Ć' | 'Č' => 'C',
        'è'..='ë' | 'ē' | 'ę' | 'ě' => 'e',
        'È'..='Ë' | 'Ē' | 'Ę' | 'Ě' => 'E',
        'ì'..='ï' | 'ī' => 'i',
        'Ì'..='Ï' | 'Ī' => 'I',
        'ñ' | 'ń' | 'ň' => 'n',
        'Ñ' | 'Ń' | 'Ň' => 'N',
        'ò'..='ö' | 'ø' | 'ō' | 'ő' => 'o',
        'Ò'..='Ö' | 'Ø' | 'Ō' | 'Ő' => 'O',
        'ù'..='ü' | 'ū' | 'ů' | 'ű' => 'u',
        'Ù'..='Ü' | 'Ū' | 'Ů' | 'Ű' => 'U',
        'ý' | 'ÿ' => 'y',
        'Ý' => 'Y',
        'ś' | 'š' => 's',
        'Ś' | 'Š' => 'S',
        'ź' | 'ż' | 'ž' => 'z',
        'Ź' | 'Ż' | 'Ž' => 'Z',
        'ł' => 'l',
        'Ł' => 'L',
        other => other,
    };
    s
}

/// Comparação "humana": letras primeiro sem acento/caixa, depois acento, depois caixa.
pub fn collate(a: &str, b: &str, numeric: bool, sensitivity: &str) -> Ordering {
    fn tokens(s: &str, numeric: bool) -> Vec<(Option<f64>, String)> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut digits = String::new();
        for c in s.chars() {
            if numeric && c.is_ascii_digit() {
                if !cur.is_empty() {
                    out.push((None, std::mem::take(&mut cur)));
                }
                digits.push(c);
            } else {
                if !digits.is_empty() {
                    out.push((digits.parse().ok(), std::mem::take(&mut digits)));
                }
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            out.push((None, cur));
        }
        if !digits.is_empty() {
            out.push((digits.parse().ok(), digits));
        }
        out
    }
    let key = |s: &str, level: u8| -> String {
        s.chars()
            .filter(|c| !matches!(c, '\u{300}'..='\u{36f}'))
            .map(|c| match level {
                0 => base_char(c).to_lowercase().next().unwrap_or(c),
                1 => c.to_lowercase().next().unwrap_or(c),
                _ => c,
            })
            .collect()
    };
    let (ta, tb) = (tokens(a, numeric), tokens(b, numeric));
    // nível primário (com números comparados pelo valor)
    for (x, y) in ta.iter().zip(tb.iter()) {
        let o = match (x.0, y.0) {
            (Some(p), Some(q)) => p.partial_cmp(&q).unwrap_or(Ordering::Equal),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            _ => {
                let (kx, ky) = (key(&x.1, 0), key(&y.1, 0));
                let ox = kx.chars().map(|c| (!c.is_alphanumeric(), c));
                let oy = ky.chars().map(|c| (!c.is_alphanumeric(), c));
                ox.cmp(oy)
            }
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    let o = ta.len().cmp(&tb.len());
    if o != Ordering::Equal || sensitivity == "base" {
        return o;
    }
    // acentos
    let o = key(a, 1).cmp(&key(b, 1));
    if o != Ordering::Equal || sensitivity == "accent" {
        return o;
    }
    // caixa: minúscula antes
    for (x, y) in a.chars().zip(b.chars()) {
        if x != y {
            if x.is_lowercase() && y.is_uppercase() {
                return Ordering::Less;
            }
            if x.is_uppercase() && y.is_lowercase() {
                return Ordering::Greater;
            }
        }
    }
    a.cmp(b)
}

