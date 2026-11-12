//! Ligação do motor JavaScript (Ferro JS) com o sistema: idioma padrão e
//! fusos horários (via `jiff`, com a base de fusos IANA embutida).

use ferro_js::vm::TzInfo;

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
    engine.vm.tz = Box::new(move |zone, t| {
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
