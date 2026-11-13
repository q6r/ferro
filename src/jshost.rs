//! Ligação do motor JavaScript (Ferro JS) com o sistema: idioma padrão,
//! fusos horários (via `jiff`, com a base de fusos IANA embutida) e módulos
//! ES lidos do disco (linha de comando).

mod tzlinks;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ferro_js::module::ModuleHost;
use ferro_js::value::{ObjRef, Property};
use ferro_js::vm::{TzHost, TzInfo, Vm};
use ferro_js::{JsStr, Value};
use jiff::Timestamp;
use jiff::tz::{AmbiguousOffset, TimeZone};
use url::Url;

/// Idioma do sistema (ex.: "pt-BR"). `FERRO_LOCALE` força outro.
pub fn system_locale() -> String {
    if let Ok(l) = std::env::var("FERRO_LOCALE")
        && !l.is_empty()
    {
        return l;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buf = [0u16; 85];
        // SEGURANÇA: o buffer tem o tamanho informado; a função escreve no máximo isso.
        let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
        if n > 1 {
            return String::from_utf16_lossy(&buf[..(n - 1) as usize]);
        }
    }
    std::env::var("LANG")
        .ok()
        .and_then(|l| l.split('.').next().map(|s| s.replace('_', "-")))
        .filter(|s| !s.is_empty() && s != "C" && s != "POSIX")
        .unwrap_or_else(|| "en-US".into())
}

/// Nome longo do fuso, como o navegador mostra em `Date.prototype.toString`.
fn long_name(iana: &str, dst: bool, offset_min: f64, pt: bool) -> String {
    let name = match (iana, dst, pt) {
        ("America/Sao_Paulo" | "America/Bahia" | "America/Fortaleza" | "America/Recife" | "America/Belem", _, true) => {
            "Horário Padrão de Brasília"
        }
        ("America/Sao_Paulo" | "America/Bahia" | "America/Fortaleza" | "America/Recife" | "America/Belem", _, false) => {
            "Brasilia Standard Time"
        }
        ("America/Manaus" | "America/Cuiaba" | "America/Porto_Velho", _, true) => "Horário Padrão do Amazonas",
        ("America/Manaus" | "America/Cuiaba" | "America/Porto_Velho", _, false) => "Amazon Standard Time",
        ("UTC" | "Etc/UTC", _, true) => "Tempo Universal Coordenado",
        ("UTC" | "Etc/UTC", _, false) => "Coordinated Universal Time",
        ("America/New_York", false, false) => "Eastern Standard Time",
        ("America/New_York", true, false) => "Eastern Daylight Time",
        ("America/Chicago", false, false) => "Central Standard Time",
        ("America/Chicago", true, false) => "Central Daylight Time",
        ("America/Los_Angeles", false, false) => "Pacific Standard Time",
        ("America/Los_Angeles", true, false) => "Pacific Daylight Time",
        ("Europe/London", false, false) => "Greenwich Mean Time",
        ("Europe/London", true, false) => "British Summer Time",
        ("Europe/Lisbon", false, true) => "Hora padrão da Europa Ocidental",
        ("Europe/Lisbon", true, true) => "Hora de verão da Europa Ocidental",
        _ => "",
    };
    if !name.is_empty() {
        return name.into();
    }
    let m = offset_min as i64;
    let sign = if m < 0 { '-' } else { '+' };
    format!("GMT{sign}{:02}:{:02}", m.abs() / 60, m.abs() % 60)
}

/// Configura um motor JS com o idioma e o fuso do sistema.
pub fn setup(engine: &mut ferro_js::Engine) {
    let locale = system_locale();
    let pt = locale.starts_with("pt");
    engine.vm.locale = locale;
    let local = jiff::tz::TimeZone::system();
    engine.vm.zones = Box::new(JiffZones::new(local.clone()));
    engine.vm.tz = Box::new(move |zone, t| {
        if let Some(m) = zone.and_then(offset_zone) {
            let sign = if m < 0 { '-' } else { '+' };
            let name = format!("{sign}{:02}:{:02}", m.abs() / 60, m.abs() % 60);
            return Some(TzInfo {
                offset_min: m as f64,
                long: format!("GMT{name}"),
                short: name,
            });
        }
        let tz = match zone {
            None => local.clone(),
            Some(z) => jiff::tz::TimeZone::get(z).ok()?,
        };
        let ms = if t.is_finite() { t.clamp(-8.64e15, 8.64e15) as i64 } else { 0 };
        let ts = jiff::Timestamp::from_millisecond(ms).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
        let info = tz.to_offset_info(ts);
        let offset_min = info.offset().seconds() as f64 / 60.0;
        let short = info.abbreviation().to_string();
        let long = long_name(tz.iana_name().unwrap_or(""), info.dst().is_dst(), offset_min, pt);
        Some(TzInfo { offset_min, short, long })
    });
}

/// Fuso de deslocamento fixo ("+05:30", "-0300", "+01"): minutos.
fn offset_zone(z: &str) -> Option<i32> {
    let b = z.as_bytes();
    let sign = match b.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits: String = z[1..].chars().filter(|c| *c != ':').collect();
    if !digits.bytes().all(|c| c.is_ascii_digit()) || !(digits.len() == 2 || digits.len() == 4) || (z.len() == 6 && b[3] != b':') {
        return None;
    }
    let h: i32 = digits[..2].parse().ok()?;
    let m: i32 = if digits.len() == 4 { digits[2..].parse().ok()? } else { 0 };
    (h < 24 && m < 60).then_some(sign * (h * 60 + m))
}

/// Fusos do Temporal com a base IANA do `jiff`.
struct JiffZones {
    system: TimeZone,
    zones: RefCell<HashMap<String, TimeZone>>,
    /// Nome em minúsculas → (identificador, primário).
    names: RefCell<HashMap<String, Option<(String, String)>>>,
}

const NS: i128 = 1_000_000_000;

impl JiffZones {
    fn new(system: TimeZone) -> JiffZones {
        JiffZones {
            system,
            zones: RefCell::new(HashMap::new()),
            names: RefCell::new(HashMap::new()),
        }
    }

    fn zone(&self, id: &str) -> Option<TimeZone> {
        if let Some(z) = self.zones.borrow().get(id) {
            return Some(z.clone());
        }
        let z = TimeZone::get(id).ok()?;
        self.zones.borrow_mut().insert(id.to_string(), z.clone());
        Some(z)
    }

    /// Instante do jiff (limitado aos anos -9999..9999 que ele aceita).
    fn ts(ns: i128) -> Timestamp {
        let ns = ns.clamp(Timestamp::MIN.as_nanosecond(), Timestamp::MAX.as_nanosecond());
        Timestamp::from_nanosecond(ns).unwrap_or(Timestamp::UNIX_EPOCH)
    }

    fn offset_at(tz: &TimeZone, ns: i128) -> i128 {
        tz.to_offset(Self::ts(ns)).seconds() as i128 * NS
    }
}

impl TzHost for JiffZones {
    fn resolve(&self, name: &str) -> Option<(String, String)> {
        if name.eq_ignore_ascii_case("UTC") {
            return Some(("UTC".into(), "UTC".into()));
        }
        let key = name.to_ascii_lowercase();
        if let Some(r) = self.names.borrow().get(&key) {
            return r.clone();
        }
        let r = TimeZone::get(name).ok().and_then(|tz| {
            let id = tz.iana_name()?.to_string();
            let primary = match tzlinks::TZ_LINKS.binary_search_by(|(n, _)| (*n).cmp(id.as_str())) {
                Ok(i) => tzlinks::TZ_LINKS[i].1.to_string(),
                Err(_) => id.clone(),
            };
            Some((id, primary))
        });
        self.names.borrow_mut().insert(key, r.clone());
        r
    }

    fn offset_ns(&self, id: &str, epoch_ns: i128) -> i64 {
        self.zone(id).map_or(0, |tz| Self::offset_at(&tz, epoch_ns) as i64)
    }

    fn possible(&self, id: &str, local_ns: i128) -> Vec<i128> {
        let Some(tz) = self.zone(id) else { return vec![local_ns] };
        let Ok(t) = Timestamp::from_nanosecond(local_ns) else {
            // fora do alcance do jiff: o deslocamento da ponta
            return vec![local_ns - Self::offset_at(&tz, local_ns)];
        };
        let dt = jiff::tz::Offset::UTC.to_datetime(t);
        match tz.to_ambiguous_timestamp(dt).offset() {
            AmbiguousOffset::Unambiguous { offset } => vec![local_ns - offset.seconds() as i128 * NS],
            AmbiguousOffset::Gap { .. } => Vec::new(),
            AmbiguousOffset::Fold { before, after } => {
                let mut v = vec![local_ns - before.seconds() as i128 * NS, local_ns - after.seconds() as i128 * NS];
                v.sort();
                v
            }
        }
    }

    fn transition(&self, id: &str, epoch_ns: i128, next: bool) -> Option<i128> {
        let tz = self.zone(id)?;
        let start = Self::ts(epoch_ns);
        // só conta mudança de deslocamento (não de nome ou de horário de verão)
        let changed = |t: Timestamp, off: jiff::tz::Offset| Self::offset_at(&tz, t.as_nanosecond() - 1) != off.seconds() as i128 * NS;
        if next {
            tz.following(start)
                .take(64)
                .find(|tr| changed(tr.timestamp(), tr.offset()))
                .map(|tr| tr.timestamp().as_nanosecond())
        } else {
            tz.preceding(start)
                .take(64)
                .find(|tr| changed(tr.timestamp(), tr.offset()))
                .map(|tr| tr.timestamp().as_nanosecond())
        }
    }

    fn system(&self) -> String {
        if let Some(name) = self.system.iana_name()
            && let Some((_, primary)) = self.resolve(name)
        {
            return primary;
        }
        let off = self.system.to_offset(Timestamp::now()).seconds();
        if off == 0 || off % 60 != 0 {
            return "UTC".into();
        }
        let m = off / 60;
        format!("{}{:02}:{:02}", if m < 0 { '-' } else { '+' }, m.abs() / 60, m.abs() % 60)
    }
}

// ------------------------------------------------------------ módulos (CLI)

/// Estado da linha de comando: a base para especificadores de quem não é
/// um arquivo (entrada padrão sem `--caminho`, REPL, eval).
struct CliHost {
    base: Url,
}

/// Liga os módulos ES ao disco: especificadores relativos (`./x.js`,
/// `../y.mjs`, `/z.js`) ou URLs `file:` resolvidos contra o arquivo de quem
/// importa; `base` vale para quem não tem arquivo.
pub fn setup_modules(engine: &mut ferro_js::Engine, base: Url) {
    engine.vm.host = Some(Rc::new(CliHost { base }) as Rc<dyn std::any::Any>);
    engine.vm.modules.host = Some(ModuleHost {
        resolve: cli_resolve,
        fetch: cli_fetch,
        meta: cli_meta,
    });
}

/// URL `file:` de um caminho (relativo ao diretório atual).
pub fn file_url(path: &std::path::Path) -> Option<Url> {
    let abs = std::path::absolute(path).ok()?;
    Url::from_file_path(abs).ok()
}

fn cli_base(vm: &Vm) -> Url {
    vm.host
        .as_ref()
        .and_then(|h| h.downcast_ref::<CliHost>())
        .map(|h| h.base.clone())
        .or_else(|| std::env::current_dir().ok().and_then(|d| Url::from_directory_path(d).ok()))
        .unwrap_or_else(|| Url::parse("file:///").expect("URL fixa"))
}

/// Resolve como o navegador: relativo ao módulo/script de quem importa.
fn resolve_specifier(vm: &Vm, spec: &str, referrer: &str) -> Result<String, String> {
    let base = Url::parse(referrer).ok().filter(|u| u.scheme() == "file").unwrap_or_else(|| cli_base(vm));
    if spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') {
        return base.join(spec).map(String::from).map_err(|e| e.to_string());
    }
    if let Ok(u) = Url::parse(spec) {
        return Ok(u.into());
    }
    Err(format!(
        "Failed to resolve module specifier \"{spec}\". Relative references must start with either \"/\", \"./\", or \"../\"."
    ))
}

fn cli_resolve(vm: &mut Vm, spec: &str, referrer: &str) -> Result<String, String> {
    resolve_specifier(vm, spec, referrer)
}

fn cli_fetch(_vm: &mut Vm, url: &str) -> Option<ferro_js::module::Fetched> {
    let path = Url::parse(url).ok().and_then(|u| u.to_file_path().ok());
    Some(match path {
        Some(p) => std::fs::read(&p).map_err(|_| format!("Error reading module from {}", p.display())),
        None => Err(format!("Error reading module from {url}")),
    })
}

/// `import.meta`: `url` e `resolve(especificador)`.
fn cli_meta(vm: &mut Vm, url: &str, meta: &ObjRef) {
    let resolve = vm.realm.native("resolve", 1, meta_resolve);
    ferro_js::builtins::set_native_data(&resolve, Value::str(url));
    let mut b = meta.borrow_mut();
    b.props.insert(ferro_js::value::PropKey::str("url"), Property::data(Value::str(url)));
    b.props.insert(ferro_js::value::PropKey::str("resolve"), Property::data(Value::Object(resolve)));
}

fn meta_resolve(vm: &mut Vm, a: &mut ferro_js::value::CallArgs) -> Result<Value, Value> {
    let spec = vm.to_string(&a.arg(0))?.to_rust();
    let referrer = match ferro_js::builtins::native_data(&a.callee) {
        Value::String(s) => s.to_rust(),
        _ => String::new(),
    };
    match resolve_specifier(vm, &spec, &referrer) {
        Ok(u) => Ok(Value::String(JsStr::new(&u))),
        Err(msg) => Err(vm.type_error(msg)),
    }
}
