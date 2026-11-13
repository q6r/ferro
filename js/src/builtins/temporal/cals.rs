//! Calendários não ISO do ECMA-402: conversão entre o dia (contado desde
//! 1970-01-01) e a data do calendário, meses (códigos e intercalares),
//! tamanhos e eras. Os aritméticos (gregoriano e derivados, copta, etíope,
//! indiano, islâmico tabular, persa, hebraico) são calculados; chinês,
//! coreano e Umm al-Qura usam tabelas geradas do ICU.

use super::cal_data;
use super::calendar::Cal;
use super::iso;

/// Data num calendário: ano aritmético, mês ordinal (1..13) e dia.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CDate {
    pub year: i64,
    pub month: u8,
    pub day: u8,
}

/// Código de mês: número e intercalar ("M05L"). A ordem é a dos textos.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct MonthCode {
    pub num: u8,
    pub leap: bool,
}

impl MonthCode {
    pub fn plain(num: u8) -> MonthCode {
        MonthCode { num, leap: false }
    }

    pub fn string(self) -> String {
        format!("M{:02}{}", self.num, if self.leap { "L" } else { "" })
    }
}

/// Uma era: código, apelidos, data (no calendário) em que começa e o número
/// do primeiro ano. `reverse`: conta os anos para trás (como a.C.).
pub struct Era {
    pub code: &'static str,
    pub aliases: &'static [&'static str],
    pub anchor: (i64, u8, u8),
    pub start: i64,
    pub reverse: bool,
}

const fn era(code: &'static str, aliases: &'static [&'static str], anchor: (i64, u8, u8), start: i64, reverse: bool) -> Era {
    Era {
        code,
        aliases,
        anchor,
        start,
        reverse,
    }
}

const E1: (i64, u8, u8) = (1, 1, 1);
static GREGORY_ERAS: [Era; 2] = [era("ce", &["ad"], E1, 1, false), era("bce", &["bc"], E1, 1, true)];
static JAPANESE_ERAS: [Era; 7] = [
    era("reiwa", &[], (2019, 5, 1), 1, false),
    era("heisei", &[], (1989, 1, 8), 1, false),
    era("showa", &[], (1926, 12, 25), 1, false),
    era("taisho", &[], (1912, 7, 30), 1, false),
    era("meiji", &[], (1873, 1, 1), 6, false),
    era("ce", &["ad"], E1, 1, false),
    era("bce", &["bc"], E1, 1, true),
];
static ROC_ERAS: [Era; 2] = [era("roc", &["minguo"], E1, 1, false), era("broc", &["before-roc", "minguo-qian"], E1, 1, true)];
static BUDDHIST_ERAS: [Era; 1] = [era("be", &[], E1, 1, false)];
static COPTIC_ERAS: [Era; 1] = [era("am", &[], E1, 1, false)];
static ETHIOPIC_ERAS: [Era; 2] = [era("am", &["incar"], E1, 1, false), era("aa", &["mundi"], (-5499, 1, 1), 1, false)];
static ETHIOAA_ERAS: [Era; 1] = [era("aa", &["mundi"], E1, 1, false)];
static INDIAN_ERAS: [Era; 1] = [era("shaka", &[], E1, 1, false)];
static ISLAMIC_ERAS: [Era; 2] = [era("ah", &[], E1, 1, false), era("bh", &[], E1, 1, true)];
static PERSIAN_ERAS: [Era; 1] = [era("ap", &[], E1, 1, false)];
static HEBREW_ERAS: [Era; 1] = [era("am", &[], E1, 1, false)];

pub fn eras(cal: Cal) -> &'static [Era] {
    match cal {
        Cal::Gregory => &GREGORY_ERAS,
        Cal::Japanese => &JAPANESE_ERAS,
        Cal::Roc => &ROC_ERAS,
        Cal::Buddhist => &BUDDHIST_ERAS,
        Cal::Coptic => &COPTIC_ERAS,
        Cal::Ethiopic => &ETHIOPIC_ERAS,
        Cal::Ethioaa => &ETHIOAA_ERAS,
        Cal::Indian => &INDIAN_ERAS,
        Cal::IslamicCivil | Cal::IslamicTbla | Cal::IslamicUmalqura => &ISLAMIC_ERAS,
        Cal::Persian => &PERSIAN_ERAS,
        Cal::Hebrew => &HEBREW_ERAS,
        _ => &[],
    }
}

/// O código canônico de uma era (aceita os apelidos).
pub fn canonical_era(cal: Cal, s: &str) -> Option<&'static Era> {
    eras(cal).iter().find(|e| e.code == s || e.aliases.contains(&s))
}

/// eraFromYear: a era e o ano da era de uma data.
pub fn era_of(cal: Cal, year: i64, month: u8, day: u8) -> Option<(&'static str, i64)> {
    let list = eras(cal);
    for (i, e) in list.iter().enumerate() {
        if i == list.len() - 1 {
            let ey = if e.reverse { e.anchor.0 - year } else { year - e.anchor.0 + e.start };
            return Some((e.code, ey));
        }
        if (year, month, day) >= e.anchor {
            return Some((e.code, year - e.anchor.0 + e.start));
        }
    }
    None
}

/// Ano aritmético a partir da era e do ano da era.
pub fn year_of_era(e: &Era, era_year: i64) -> i64 {
    if e.reverse { e.anchor.0 - era_year } else { era_year + e.anchor.0 - e.start }
}

/// Menor e maior ano aceitos (calendarMinYear/MaxYear da referência).
pub fn year_range(cal: Cal) -> (i64, i64) {
    match cal {
        Cal::Buddhist => (-271278, 276303),
        Cal::Coptic => (-272099, 275471),
        Cal::Ethioaa => (-266323, 281247),
        Cal::Ethiopic => (-271823, 275747),
        Cal::Hebrew => (-268058, 279517),
        Cal::Indian => (-271899, 275682),
        Cal::IslamicCivil | Cal::IslamicTbla | Cal::IslamicUmalqura => (-280804, 283583),
        Cal::Persian => (-272442, 275139),
        Cal::Roc => (-273732, 273849),
        _ => (-271821, 275760),
    }
}

// ------------------------------------------------------- gregorianos

/// Ano do calendário − ano ISO nos calendários com meses e dias gregorianos.
fn greg_offset(cal: Cal) -> Option<i64> {
    match cal {
        Cal::Gregory | Cal::Japanese => Some(0),
        Cal::Buddhist => Some(543),
        Cal::Roc => Some(-1911),
        _ => None,
    }
}

// ------------------------------------------- copta e etíopes (juliano)

fn orthodox_epoch(cal: Cal) -> i64 {
    match cal {
        Cal::Coptic => iso::epoch_days(284, 8, 29),
        _ => iso::epoch_days(8, 8, 27),
    }
}

/// Ano do calendário → ano da conta copta/etíope (Amete Mihret).
fn orthodox_year(cal: Cal, y: i64) -> i64 {
    if cal == Cal::Ethioaa { y - 5500 } else { y }
}

fn orthodox_days(cal: Cal, y: i64, m: u8, d: u8) -> i64 {
    let y = orthodox_year(cal, y);
    orthodox_epoch(cal) - 1 + 365 * (y - 1) + y.div_euclid(4) + 30 * (m as i64 - 1) + d as i64
}

fn orthodox_from(cal: Cal, z: i64) -> CDate {
    let ep = orthodox_epoch(cal);
    let y = (4 * (z - ep) + 1463).div_euclid(1461);
    let start = ep - 1 + 365 * (y - 1) + y.div_euclid(4);
    let doy = z - start - 1;
    let m = (doy / 30 + 1) as u8;
    let d = (doy - 30 * (m as i64 - 1) + 1) as u8;
    let year = if cal == Cal::Ethioaa { y + 5500 } else { y };
    CDate { year, month: m, day: d }
}

fn orthodox_leap(cal: Cal, y: i64) -> bool {
    orthodox_year(cal, y).rem_euclid(4) == 3
}

// ------------------------------------------------------------- indiano

/// Início de cada mês (mês e dia ISO; os dois últimos no ano ISO seguinte).
const INDIAN_STARTS: [(u8, u8); 12] = [(3, 22), (4, 21), (5, 22), (6, 22), (7, 23), (8, 23), (9, 23), (10, 23), (11, 22), (12, 22), (1, 21), (2, 20)];

fn indian_leap(y: i64) -> bool {
    iso::is_leap(y + 78)
}

fn indian_days(y: i64, m: u8, d: u8) -> i64 {
    let iy = y + 78 + if m >= 11 { 1 } else { 0 };
    let (im, mut id) = INDIAN_STARTS[m as usize - 1];
    if m == 1 && indian_leap(y) {
        id = 21;
    }
    iso::epoch_days(iy, im, id) + d as i64 - 1
}

fn indian_from(z: i64) -> CDate {
    let iso_date = iso::from_epoch_days(z);
    let mut y = iso_date.year - 78;
    if z < indian_days(y, 1, 1) {
        y -= 1;
    }
    let mut m = 12;
    while m > 1 && indian_days(y, m, 1) > z {
        m -= 1;
    }
    CDate {
        year: y,
        month: m,
        day: (z - indian_days(y, m, 1) + 1) as u8,
    }
}

// ------------------------------------------------------------ islâmico

const ISLAMIC_CIVIL_EPOCH: i64 = 227015 - 719163;

fn islamic_epoch(cal: Cal) -> i64 {
    if cal == Cal::IslamicTbla { ISLAMIC_CIVIL_EPOCH - 1 } else { ISLAMIC_CIVIL_EPOCH }
}

fn tabular_days(ep: i64, y: i64, m: u8, d: u8) -> i64 {
    let m = m as i64;
    d as i64 + 29 * (m - 1) + (6 * m - 1) / 11 + (y - 1) * 354 + (3 + 11 * y).div_euclid(30) + ep - 1
}

fn tabular_from(ep: i64, z: i64) -> CDate {
    let y = (30 * (z - ep) + 10646).div_euclid(10631);
    let prior = z - tabular_days(ep, y, 1, 1);
    let m = ((11 * prior + 330) / 325) as u8;
    CDate {
        year: y,
        month: m,
        day: (z - tabular_days(ep, y, m, 1) + 1) as u8,
    }
}

fn umalqura_entry(y: i64) -> Option<(i64, u16)> {
    let i = y - 1300;
    if (0..cal_data::UMALQURA.len() as i64).contains(&i) {
        let (s, m) = cal_data::UMALQURA[i as usize];
        Some((s as i64, m))
    } else {
        None
    }
}

fn umalqura_days(y: i64, m: u8, d: u8) -> i64 {
    match umalqura_entry(y) {
        Some((start, mask)) => {
            let mut z = start;
            for k in 0..(m as usize - 1) {
                z += if mask & (1 << k) != 0 { 30 } else { 29 };
            }
            z + d as i64 - 1
        }
        None => tabular_days(ISLAMIC_CIVIL_EPOCH, y, m, d),
    }
}

fn umalqura_from(z: i64) -> CDate {
    let first = cal_data::UMALQURA[0].0 as i64;
    let last = cal_data::UMALQURA.len() - 1;
    let end = umalqura_days(1300 + last as i64 + 1, 1, 1);
    if z < first || z >= end {
        return tabular_from(ISLAMIC_CIVIL_EPOCH, z);
    }
    let i = cal_data::UMALQURA.partition_point(|&(s, _)| (s as i64) <= z) - 1;
    let (start, mask) = cal_data::UMALQURA[i];
    let mut rest = z - start as i64;
    let mut m = 1u8;
    loop {
        let len = if mask & (1 << (m - 1)) != 0 { 30 } else { 29 };
        if rest < len || m == 12 {
            break;
        }
        rest -= len;
        m += 1;
    }
    CDate {
        year: 1300 + i as i64,
        month: m,
        day: rest as u8 + 1,
    }
}

// --------------------------------------------------------------- persa

const PERSIAN_EPOCH: i64 = 1948321 - 2440588;

fn persian_new_year(y: i64) -> i64 {
    let a = PERSIAN_EPOCH - 1 + 365 * (y - 1) + (8 * y + 21).div_euclid(33);
    if cal_data::PERSIAN_EARLY.binary_search(&(y as i32)).is_ok() { a - 1 } else { a }
}

fn persian_days(y: i64, m: u8, d: u8) -> i64 {
    let m = m as i64;
    let before = if m <= 7 { 31 * (m - 1) } else { 30 * (m - 1) + 6 };
    persian_new_year(y) + before + d as i64 - 1
}

fn persian_from(z: i64) -> CDate {
    let mut y = ((z - PERSIAN_EPOCH) as f64 / 365.2422).floor() as i64 + 1;
    while persian_new_year(y) > z {
        y -= 1;
    }
    while persian_new_year(y + 1) <= z {
        y += 1;
    }
    let doy = z - persian_new_year(y);
    let m = if doy < 186 { doy / 31 + 1 } else { (doy - 6) / 30 + 1 };
    let d = doy - if m <= 7 { 31 * (m - 1) } else { 30 * (m - 1) + 6 } + 1;
    CDate {
        year: y,
        month: m as u8,
        day: d as u8,
    }
}

// ------------------------------------------------------------ hebraico

const HEBREW_EPOCH: i64 = -1373427 - 719163;

fn hebrew_leap(y: i64) -> bool {
    (7 * y + 1).rem_euclid(19) < 7
}

fn hebrew_elapsed(y: i64) -> i64 {
    let months = (235 * y - 234).div_euclid(19);
    let parts = 12084 + 13753 * months;
    let days = 29 * months + parts.div_euclid(25920);
    if (3 * (days + 1)).rem_euclid(7) < 3 { days + 1 } else { days }
}

fn hebrew_new_year(y: i64) -> i64 {
    let (a, b, c) = (hebrew_elapsed(y - 1), hebrew_elapsed(y), hebrew_elapsed(y + 1));
    let corr = if c - b == 356 {
        2
    } else if b - a == 382 {
        1
    } else {
        0
    };
    HEBREW_EPOCH + b + corr
}

fn hebrew_year_days(y: i64) -> i64 {
    hebrew_new_year(y + 1) - hebrew_new_year(y)
}

fn hebrew_month_len(y: i64, m: u8) -> u8 {
    let leap = hebrew_leap(y);
    let len = hebrew_year_days(y);
    match (m, leap) {
        (1, _) => 30,
        (2, _) => {
            if len % 10 == 5 {
                30
            } else {
                29
            }
        }
        (3, _) => {
            if len % 10 == 3 {
                29
            } else {
                30
            }
        }
        (4, _) => 29,
        (5, _) => 30,
        (6, true) => 30,
        (6, false) => 29,
        (m, true) => {
            if m % 2 == 1 {
                29
            } else {
                30
            }
        }
        (m, false) => {
            if m % 2 == 1 {
                30
            } else {
                29
            }
        }
    }
}

fn hebrew_days(y: i64, m: u8, d: u8) -> i64 {
    let mut z = hebrew_new_year(y);
    for k in 1..m {
        z += hebrew_month_len(y, k) as i64;
    }
    z + d as i64 - 1
}

fn hebrew_from(z: i64) -> CDate {
    let mut y = ((z - HEBREW_EPOCH) as f64 / 365.2468).floor() as i64 + 1;
    while hebrew_new_year(y) > z {
        y -= 1;
    }
    while hebrew_new_year(y + 1) <= z {
        y += 1;
    }
    let mut rest = z - hebrew_new_year(y);
    let mut m = 1u8;
    loop {
        let len = hebrew_month_len(y, m) as i64;
        if rest < len {
            break;
        }
        rest -= len;
        m += 1;
    }
    CDate {
        year: y,
        month: m,
        day: rest as u8 + 1,
    }
}

// ---------------------------------------------------- chinês e coreano

fn lunar_table(cal: Cal) -> &'static [(i32, u32)] {
    if cal == Cal::Dangi { &cal_data::DANGI } else { &cal_data::CHINESE }
}

fn lunar_in_table(y: i64) -> bool {
    (cal_data::LUNAR_FIRST..cal_data::LUNAR_FIRST + cal_data::CHINESE.len() as i64).contains(&y)
}

/// Anos fora da tabela: deslocados em ciclos de 19 anos (ciclo metônico)
/// para dentro dela.
fn metonic(y: i64) -> i64 {
    (y - 1991).div_euclid(19) * 19
}

/// O ano equivalente dentro da tabela.
fn lunar_year(y: i64) -> i64 {
    if lunar_in_table(y) { y } else { y - metonic(y) }
}

/// (início, máscara) de um ano da tabela.
fn lunar_entry(cal: Cal, y: i64) -> (i64, u32) {
    let t = lunar_table(cal);
    let i = (y - cal_data::LUNAR_FIRST).clamp(0, t.len() as i64 - 1) as usize;
    (t[i].0 as i64, t[i].1)
}

fn lunar_months(cal: Cal, y: i64) -> u8 {
    let (_, mask) = lunar_entry(cal, lunar_year(y));
    if mask >> 13 != 0 { 13 } else { 12 }
}

fn lunar_len(cal: Cal, y: i64, m: u8) -> u8 {
    let (_, mask) = lunar_entry(cal, lunar_year(y));
    if mask & (1 << (m - 1)) != 0 { 30 } else { 29 }
}

fn lunar_code(cal: Cal, y: i64, m: u8) -> MonthCode {
    let (_, mask) = lunar_entry(cal, lunar_year(y));
    let leap = (mask >> 13) as u8;
    if leap == 0 || m <= leap {
        MonthCode::plain(m)
    } else if m == leap + 1 {
        MonthCode { num: leap, leap: true }
    } else {
        MonthCode::plain(m - 1)
    }
}

/// Dia ISO deslocado em `shift` anos (29/2 vira 28/2 quando preciso).
fn shift_iso_days(z: i64, shift: i64) -> i64 {
    if shift == 0 {
        return z;
    }
    let d = iso::from_epoch_days(z);
    let y = d.year + shift;
    let day = d.day.min(iso::days_in_month(y, d.month));
    iso::epoch_days(y, d.month, day)
}

fn lunar_days(cal: Cal, y: i64, m: u8, d: u8) -> i64 {
    let shift = if lunar_in_table(y) { 0 } else { metonic(y) };
    let (start, mask) = lunar_entry(cal, y - shift);
    let mut z = start;
    for k in 0..(m - 1) {
        z += if mask & (1 << k) != 0 { 30 } else { 29 };
    }
    shift_iso_days(z + d as i64 - 1, shift)
}

fn lunar_from(cal: Cal, z: i64) -> CDate {
    let t = lunar_table(cal);
    let first = t[0].0 as i64;
    let last = t.len() - 1;
    let end = t[last].0 as i64 + if t[last].1 >> 13 != 0 { 384 } else { 354 };
    let (z2, shift) = if (first..end).contains(&z) {
        (z, 0)
    } else {
        let shift = metonic(iso::from_epoch_days(z).year);
        (shift_iso_days(z, -shift), shift)
    };
    let i = t.partition_point(|&(s, _)| (s as i64) <= z2).saturating_sub(1);
    let (start, mask) = t[i];
    let year = cal_data::LUNAR_FIRST + i as i64;
    let mut rest = (z2 - start as i64).max(0);
    let months = if mask >> 13 != 0 { 13 } else { 12 };
    let mut m = 1u8;
    loop {
        let len = if mask & (1 << (m - 1)) != 0 { 30 } else { 29 };
        if rest < len || m == months {
            break;
        }
        rest -= len;
        m += 1;
    }
    CDate {
        year: year + shift,
        month: m,
        day: (rest + 1).min(30) as u8,
    }
}

// ------------------------------------------------------------- comuns

pub fn months_in_year(cal: Cal, y: i64) -> u8 {
    match cal {
        Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => 13,
        Cal::Hebrew => {
            if hebrew_leap(y) {
                13
            } else {
                12
            }
        }
        Cal::Chinese | Cal::Dangi => lunar_months(cal, y),
        _ => 12,
    }
}

pub fn days_in_month(cal: Cal, y: i64, m: u8) -> u8 {
    if let Some(off) = greg_offset(cal) {
        return iso::days_in_month(y - off, m);
    }
    match cal {
        Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => {
            if m < 13 {
                30
            } else if orthodox_leap(cal, y) {
                6
            } else {
                5
            }
        }
        Cal::Indian => match m {
            1 => {
                if indian_leap(y) {
                    31
                } else {
                    30
                }
            }
            2..=6 => 31,
            _ => 30,
        },
        Cal::Hebrew => hebrew_month_len(y, m),
        Cal::IslamicCivil | Cal::IslamicTbla | Cal::IslamicUmalqura | Cal::Persian => {
            let next = if m == 12 { to_days(cal, y + 1, 1, 1) } else { to_days(cal, y, m + 1, 1) };
            (next - to_days(cal, y, m, 1)) as u8
        }
        Cal::Chinese | Cal::Dangi => lunar_len(cal, y, m),
        _ => 31,
    }
}

/// Dia (desde 1970-01-01) de uma data válida do calendário.
pub fn to_days(cal: Cal, y: i64, m: u8, d: u8) -> i64 {
    if let Some(off) = greg_offset(cal) {
        return iso::epoch_days(y - off, m, d);
    }
    match cal {
        Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => orthodox_days(cal, y, m, d),
        Cal::Indian => indian_days(y, m, d),
        Cal::IslamicCivil | Cal::IslamicTbla => tabular_days(islamic_epoch(cal), y, m, d),
        Cal::IslamicUmalqura => umalqura_days(y, m, d),
        Cal::Persian => persian_days(y, m, d),
        Cal::Hebrew => hebrew_days(y, m, d),
        Cal::Chinese | Cal::Dangi => lunar_days(cal, y, m, d),
        _ => iso::epoch_days(y, m, d),
    }
}

/// Data do calendário de um dia.
pub fn from_days(cal: Cal, z: i64) -> CDate {
    if let Some(off) = greg_offset(cal) {
        let d = iso::from_epoch_days(z);
        return CDate {
            year: d.year + off,
            month: d.month,
            day: d.day,
        };
    }
    match cal {
        Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => orthodox_from(cal, z),
        Cal::Indian => indian_from(z),
        Cal::IslamicCivil | Cal::IslamicTbla => tabular_from(islamic_epoch(cal), z),
        Cal::IslamicUmalqura => umalqura_from(z),
        Cal::Persian => persian_from(z),
        Cal::Hebrew => hebrew_from(z),
        Cal::Chinese | Cal::Dangi => lunar_from(cal, z),
        _ => {
            let d = iso::from_epoch_days(z);
            CDate {
                year: d.year,
                month: d.month,
                day: d.day,
            }
        }
    }
}

/// Código do mês ordinal `m` do ano `y`.
pub fn month_code(cal: Cal, y: i64, m: u8) -> MonthCode {
    match cal {
        Cal::Hebrew if hebrew_leap(y) => {
            if m == 6 {
                MonthCode { num: 5, leap: true }
            } else if m < 6 {
                MonthCode::plain(m)
            } else {
                MonthCode::plain(m - 1)
            }
        }
        Cal::Chinese | Cal::Dangi => lunar_code(cal, y, m),
        _ => MonthCode::plain(m),
    }
}

/// Mês ordinal de um código no ano `y` (None: não existe nesse ano).
pub fn month_of_code(cal: Cal, y: i64, mc: MonthCode) -> Option<u8> {
    (1..=months_in_year(cal, y)).find(|&m| month_code(cal, y, m) == mc)
}

pub fn in_leap_year(cal: Cal, y: i64) -> bool {
    if let Some(off) = greg_offset(cal) {
        return iso::is_leap(y - off);
    }
    match cal {
        Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => orthodox_leap(cal, y),
        Cal::Indian => indian_leap(y),
        Cal::IslamicCivil | Cal::IslamicTbla | Cal::IslamicUmalqura => days_in_year(cal, y) == 355,
        Cal::Persian => days_in_month(cal, y, 12) == 30,
        Cal::Hebrew => hebrew_leap(y),
        Cal::Chinese | Cal::Dangi => months_in_year(cal, y) == 13,
        _ => iso::is_leap(y),
    }
}

pub fn days_in_year(cal: Cal, y: i64) -> i64 {
    to_days(cal, y + 1, 1, 1) - to_days(cal, y, 1, 1)
}

/// Os códigos de mês válidos em algum ano do calendário.
pub fn valid_month_code(cal: Cal, mc: MonthCode) -> bool {
    if !mc.leap {
        return (1..=12).contains(&mc.num) || (mc.num == 13 && matches!(cal, Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa));
    }
    match cal {
        Cal::Hebrew => mc.num == 5,
        Cal::Chinese | Cal::Dangi => (1..=12).contains(&mc.num),
        _ => false,
    }
}

/// Meses do ano nos calendários de tamanho fixo (para limitar `month`).
pub fn months_per_year(cal: Cal) -> u8 {
    match cal {
        Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => 13,
        _ => 12,
    }
}

/// Ciclo (anos, meses) para somar muitos meses de uma vez.
pub fn cycle(cal: Cal) -> Option<(i64, i64)> {
    match cal {
        Cal::Chinese | Cal::Dangi => None,
        Cal::Hebrew => Some((19, 235)),
        Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => Some((1, 13)),
        _ => Some((1, 12)),
    }
}

/// Ano ISO de referência para um PlainMonthDay (monthDayISOReferenceYear);
/// None quando o mês e dia não existem em nenhum ano conhecido.
pub fn reference_year(cal: Cal, mc: MonthCode, day: u8) -> Option<i64> {
    match cal {
        Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => Some(if mc.num == 13 && day == 6 { 1971 } else { 1972 }),
        Cal::IslamicCivil | Cal::IslamicTbla => Some(if mc.num == 12 && day == 30 { 1971 } else { 1972 }),
        Cal::IslamicUmalqura => Some(if day == 30 {
            [1972, 1970, 1971, 1972, 1971, 1972, 1969, 1972, 1972, 1970, 1972, 1971][mc.num as usize - 1]
        } else {
            1972
        }),
        Cal::Hebrew => Some(if mc.leap {
            1970
        } else if (mc.num == 2 || mc.num == 3) && day == 30 {
            1971
        } else {
            1972
        }),
        Cal::Chinese | Cal::Dangi => {
            let (r29, r30) = chinese_reference(cal, mc);
            if day == 30 { r30 } else { r29 }
        }
        _ => Some(1972),
    }
}

/// Anos de referência (dia ≤ 29, dia 30) dos meses chinês/coreano.
fn chinese_reference(cal: Cal, mc: MonthCode) -> (Option<i64>, Option<i64>) {
    const N: Option<i64> = None;
    let s = |a: i64| Some(a);
    if !mc.leap {
        let r30 = match mc.num {
            1 => 1970,
            3 => {
                if cal == Cal::Dangi {
                    1968
                } else {
                    1966
                }
            }
            4 => 1970,
            6 => 1971,
            8 => 1971,
            11 => 1970,
            _ => 1972,
        };
        return (s(1972), s(r30));
    }
    match mc.num {
        2 => (s(1947), N),
        3 => (s(1966), s(1955)),
        4 => (s(1963), s(1944)),
        5 => (s(1971), s(1952)),
        6 => (s(1960), s(1941)),
        7 => (s(1968), s(1938)),
        8 => (s(1957), N),
        9 => (s(2014), N),
        10 => (s(1984), N),
        11 => (s(2034), N),
        _ => (N, N),
    }
}

/// Maior dia de um código de mês em qualquer ano (maxLengthOfAdjustedMonthCodeInAnyYear).
pub fn max_day_of_code(cal: Cal, mc: MonthCode) -> u8 {
    match cal {
        Cal::Gregory | Cal::Japanese | Cal::Buddhist | Cal::Roc => [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][mc.num as usize - 1],
        Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => {
            if mc.num == 13 {
                6
            } else {
                30
            }
        }
        Cal::Indian => {
            if mc.num <= 6 {
                31
            } else {
                30
            }
        }
        Cal::IslamicCivil | Cal::IslamicTbla => [30, 29, 30, 29, 30, 29, 30, 29, 30, 29, 30, 30][mc.num as usize - 1],
        Cal::Persian => {
            if mc.num <= 6 {
                31
            } else {
                30
            }
        }
        Cal::Hebrew => {
            if mc.leap {
                30
            } else {
                [30, 30, 30, 29, 30, 29, 30, 29, 30, 29, 30, 29][mc.num as usize - 1]
            }
        }
        _ => 30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversoes() {
        // 2020-03-01 nos calendários (valores do ICU)
        let z = iso::epoch_days(2020, 3, 1);
        let casos = [
            (Cal::Coptic, 1736, 6, 22),
            (Cal::Ethiopic, 2012, 6, 22),
            (Cal::Ethioaa, 7512, 6, 22),
            (Cal::Indian, 1941, 12, 11),
            (Cal::IslamicCivil, 1441, 7, 6),
            (Cal::IslamicTbla, 1441, 7, 7),
            (Cal::IslamicUmalqura, 1441, 7, 6),
            (Cal::Persian, 1398, 12, 11),
            (Cal::Hebrew, 5780, 6, 5),
            (Cal::Chinese, 2020, 2, 8),
            (Cal::Dangi, 2020, 2, 7),
            (Cal::Buddhist, 2563, 3, 1),
            (Cal::Roc, 109, 3, 1),
        ];
        for (cal, y, m, d) in casos {
            assert_eq!(from_days(cal, z), CDate { year: y, month: m, day: d }, "{cal:?}");
            assert_eq!(to_days(cal, y, m, d), z, "{cal:?}");
        }
        // mês intercalar: Adar I do ano hebraico bissexto e o 4º chinês de 2020
        assert_eq!(month_code(Cal::Hebrew, 5784, 6), MonthCode { num: 5, leap: true });
        assert_eq!(month_code(Cal::Chinese, 2020, 5), MonthCode { num: 4, leap: true });
        assert_eq!(era_of(Cal::Japanese, 2019, 4, 30), Some(("heisei", 31)));
        assert_eq!(era_of(Cal::Gregory, 0, 1, 1), Some(("bce", 1)));
    }
}
