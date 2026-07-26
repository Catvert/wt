//! Locale selection.
//!
//! Translations live in `locales/*.yml` and are compiled into the binary, so a release
//! is still a single file with no runtime asset to install.

use rust_i18n::available_locales;

/// Picks the locale from the environment, in decreasing order of intent:
/// `WT_LANG` (explicit), then the standard POSIX variables, then English.
///
/// `LC_ALL=fr_BE.UTF-8` and `LANG=fr` both select `fr`: only the language part matters.
pub fn detect() -> String {
    let candidates = ["WT_LANG", "LC_ALL", "LC_MESSAGES", "LANG"];
    for var in candidates {
        let Ok(value) = std::env::var(var) else {
            continue;
        };
        let lang = value
            .split(['.', '_', '@'])
            .next()
            .unwrap_or_default()
            .to_lowercase();
        // "C" and "POSIX" mean "no localisation", not a language.
        if lang.is_empty() || lang == "c" || lang == "posix" {
            continue;
        }
        if available_locales!().iter().any(|l| l == &lang) {
            return lang;
        }
    }
    "en".to_string()
}

pub fn init() {
    rust_i18n::set_locale(&detect());
}

#[cfg(test)]
mod tests {

    /// Every key must exist in every locale: a missing translation would otherwise show
    /// up as a raw key at runtime, on someone else's machine.
    #[test]
    fn locales_share_the_same_keys() {
        let en = include_str!("../locales/en.yml");
        let fr = include_str!("../locales/fr.yml");
        let keys = |src: &str| -> Vec<String> {
            let mut path: Vec<String> = Vec::new();
            let mut out = Vec::new();
            for line in src.lines() {
                let trimmed = line.trim_end();
                if trimmed.trim_start().starts_with('#') || trimmed.trim().is_empty() {
                    continue;
                }
                let indent = trimmed.len() - trimmed.trim_start().len();
                let depth = indent / 2;
                let Some((name, value)) = trimmed.trim().split_once(':') else {
                    continue;
                };
                path.truncate(depth);
                path.push(name.to_string());
                if !value.trim().is_empty() {
                    out.push(path.join("."));
                }
            }
            out.sort();
            out
        };
        let (en_keys, fr_keys) = (keys(en), keys(fr));
        let missing: Vec<_> = en_keys.iter().filter(|k| !fr_keys.contains(k)).collect();
        let extra: Vec<_> = fr_keys.iter().filter(|k| !en_keys.contains(k)).collect();
        assert!(missing.is_empty(), "missing in fr.yml: {missing:?}");
        assert!(extra.is_empty(), "unknown in fr.yml: {extra:?}");
    }
}
