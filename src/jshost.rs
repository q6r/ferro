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

