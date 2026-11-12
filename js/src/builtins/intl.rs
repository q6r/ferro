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

