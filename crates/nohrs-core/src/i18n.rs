//! Interface language.
//!
//! The Portuguese text is the key: the code reads `t!("Copiar")`, and the
//! table in [`strings`] gives the English and Spanish for it ([`extra`] the
//! French, German, Italian, Russian, Japanese and Chinese). Keeping the
//! source language in the code (rather than ids like `menu.copy`) means a
//! screen still reads naturally while being edited, and a missing entry
//! falls back to readable text instead of an id. The test at the bottom
//! scans every crate for `t!`/`tf!`/`k!` and fails when one has no row, so a
//! new string cannot ship untranslated by accident.
//!
//! The language is a process-wide setting read at render time, so changing
//! it only needs the windows to repaint.

mod arabic;
mod catalan;
mod extra;
mod hungarian;
#[cfg(not(target_os = "windows"))]
mod linux;
mod strings;

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// A language the interface is translated into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    /// Português (Brasil) — the language the code is written in.
    Pt,
    /// English — also the answer for every language without a translation.
    En,
    /// Español.
    Es,
    /// Català.
    Ca,
    /// Français.
    Fr,
    /// Deutsch.
    De,
    /// Italiano.
    It,
    /// Русский.
    Ru,
    /// 日本語.
    Ja,
    /// 简体中文.
    Zh,
    /// العربية — written right to left; the text system orders each line.
    Ar,
    /// Magyar.
    Hu,
}

impl Lang {
    /// Every language, in the order the settings page lists them.
    pub const ALL: [Lang; 12] = [
        Lang::Pt,
        Lang::En,
        Lang::Es,
        Lang::Ca,
        Lang::Fr,
        Lang::De,
        Lang::It,
        Lang::Hu,
        Lang::Ru,
        Lang::Ja,
        Lang::Zh,
        Lang::Ar,
    ];

    /// The value `ui.language` stores for this language.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Pt => "pt-BR",
            Lang::En => "en",
            Lang::Es => "es",
            Lang::Ca => "ca",
            Lang::Fr => "fr",
            Lang::De => "de",
            Lang::It => "it",
            Lang::Ru => "ru",
            Lang::Ja => "ja",
            Lang::Zh => "zh-CN",
            Lang::Ar => "ar",
            Lang::Hu => "hu",
        }
    }

    /// The language's name in itself, as a language picker shows it: someone
    /// who cannot read the current language still finds their own.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Pt => "Português",
            Lang::En => "English",
            Lang::Es => "Español",
            Lang::Ca => "Català",
            Lang::Fr => "Français",
            Lang::De => "Deutsch",
            Lang::It => "Italiano",
            Lang::Ru => "Русский",
            Lang::Ja => "日本語",
            Lang::Zh => "简体中文",
            Lang::Ar => "العربية",
            Lang::Hu => "Magyar",
        }
    }

    /// Reads a `ui.language` value or a locale name (`pt-BR`, `es_MX`, `EN`).
    /// Only the language part counts: every Portuguese reads the Brazilian
    /// text and every Spanish the same Spanish. `None` for anything else.
    pub fn parse(value: &str) -> Option<Lang> {
        let language = value.trim().split(['-', '_']).next()?.to_ascii_lowercase();
        match language.as_str() {
            "pt" => Some(Lang::Pt),
            "en" => Some(Lang::En),
            "es" => Some(Lang::Es),
            "ca" => Some(Lang::Ca),
            "fr" => Some(Lang::Fr),
            "de" => Some(Lang::De),
            "it" => Some(Lang::It),
            "ru" => Some(Lang::Ru),
            "ja" => Some(Lang::Ja),
            "zh" => Some(Lang::Zh),
            "ar" => Some(Lang::Ar),
            "hu" => Some(Lang::Hu),
            _ => None,
        }
    }

    /// The language spoken in a country (ISO 3166 two-letter code), for when
    /// the system's own language has no translation: someone in Mexico with
    /// a French regional format still most likely reads Spanish.
    pub fn for_country(country: &str) -> Option<Lang> {
        const PORTUGUESE: &[&str] = &["BR", "PT", "AO", "MZ", "CV", "GW", "ST", "TL", "MO"];
        const SPANISH: &[&str] = &[
            "ES", "MX", "AR", "CO", "CL", "PE", "VE", "EC", "GT", "CU", "BO", "DO", "HN", "PY",
            "SV", "NI", "CR", "PA", "UY", "PR", "GQ",
        ];
        const FRENCH: &[&str] = &["FR", "BE", "LU", "MC", "SN", "CI", "CM", "ML", "BF", "NE", "MG", "HT"];
        const GERMAN: &[&str] = &["DE", "AT", "CH", "LI"];
        const ITALIAN: &[&str] = &["IT", "SM", "VA"];
        const RUSSIAN: &[&str] = &["RU", "BY", "KZ", "KG"];
        const JAPANESE: &[&str] = &["JP"];
        const CHINESE: &[&str] = &["CN", "SG"];
        const HUNGARIAN: &[&str] = &["HU"];
        // Andorra: the one country where Catalan is the language of the
        // place. In Spain the language of the place is Spanish, and the one
        // of the person's own choice is read from the session.
        const CATALAN: &[&str] = &["AD"];
        const ARABIC: &[&str] = &[
            "SA", "AE", "EG", "MA", "DZ", "TN", "LY", "IQ", "JO", "LB", "SY", "KW", "QA", "BH", "OM", "YE",
            "SD", "PS", "MR",
        ];
        let country = country.trim().to_ascii_uppercase();
        let country = country.as_str();
        [
            (PORTUGUESE, Lang::Pt),
            (SPANISH, Lang::Es),
            (FRENCH, Lang::Fr),
            (GERMAN, Lang::De),
            (ITALIAN, Lang::It),
            (RUSSIAN, Lang::Ru),
            (JAPANESE, Lang::Ja),
            (CHINESE, Lang::Zh),
            (ARABIC, Lang::Ar),
            (HUNGARIAN, Lang::Hu),
            (CATALAN, Lang::Ca),
        ]
        .into_iter()
        .find(|(countries, _)| countries.contains(&country))
        .map(|(_, lang)| lang)
    }

    fn from_u8(value: u8) -> Lang {
        Lang::ALL.get(usize::from(value)).copied().unwrap_or(Lang::Pt)
    }

    fn as_u8(self) -> u8 {
        Lang::ALL.iter().position(|&lang| lang == self).unwrap_or(0) as u8
    }

    // Column of [`extra::EXTRA`] holding this language.
    fn extra_column(self) -> Option<usize> {
        match self {
            Lang::Fr => Some(0),
            Lang::De => Some(1),
            Lang::It => Some(2),
            Lang::Ru => Some(3),
            Lang::Ja => Some(4),
            Lang::Zh => Some(5),
            Lang::Pt | Lang::En | Lang::Es | Lang::Ca | Lang::Ar | Lang::Hu => None,
        }
    }
}

/// `u8::MAX` = not detected yet.
static DETECTED: AtomicU8 = AtomicU8::new(u8::MAX);
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Records the language the system asks for (see `nohrs_pages::locale`).
/// Kept apart from the preference so that going back to "auto" in the
/// settings returns to it without asking Windows again.
pub fn set_detected(lang: Lang) {
    DETECTED.store(lang.as_u8(), Ordering::Relaxed);
}

/// The language found on the system. Before detection ran — only in tests,
/// as the app detects before its first window — the code's own language:
/// a test that applies a config with "auto" must not switch the text the
/// other tests in the process compare against.
pub fn detected() -> Lang {
    match DETECTED.load(Ordering::Relaxed) {
        u8::MAX => Lang::Pt,
        value => Lang::from_u8(value),
    }
}

/// Applies `ui.language`: `"auto"` (or anything unknown) follows the system.
/// Returns whether the language changed, so the caller knows to repaint.
pub fn apply_preference(preference: &str) -> bool {
    let lang = Lang::parse(preference).unwrap_or_else(detected);
    set_language(lang)
}

/// Sets the language directly. Returns whether it changed.
pub fn set_language(lang: Lang) -> bool {
    CURRENT.swap(lang.as_u8(), Ordering::Relaxed) != lang.as_u8()
}

/// The language the interface is showing.
pub fn current() -> Lang {
    Lang::from_u8(CURRENT.load(Ordering::Relaxed))
}

fn table() -> &'static HashMap<&'static str, (&'static str, &'static str)> {
    static TABLE: OnceLock<HashMap<&'static str, (&'static str, &'static str)>> = OnceLock::new();
    TABLE.get_or_init(|| strings::STRINGS.iter().map(|&(pt, en, es)| (pt, (en, es))).collect())
}

/// `key` in the current language. A key without a row comes back as it is
/// (Portuguese); the test below keeps that from happening to literals.
pub fn tr(key: &'static str) -> &'static str {
    tr_in(current(), key)
}

fn extra_table() -> &'static HashMap<&'static str, &'static [&'static str; 6]> {
    static TABLE: OnceLock<HashMap<&'static str, &'static [&'static str; 6]>> = OnceLock::new();
    TABLE.get_or_init(|| extra::EXTRA.iter().map(|(pt, row)| (*pt, row)).collect())
}

fn arabic_table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| arabic::ARABIC.iter().copied().collect())
}

fn hungarian_table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| hungarian::HUNGARIAN.iter().copied().collect())
}

fn catalan_table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| catalan::CATALAN.iter().copied().collect())
}

/// The texts that are different on Linux, for the English, the Spanish and
/// the Catalan.
#[cfg(not(target_os = "windows"))]
fn linux_table() -> &'static HashMap<&'static str, (&'static str, &'static str, &'static str)> {
    static TABLE: OnceLock<HashMap<&'static str, (&'static str, &'static str, &'static str)>> = OnceLock::new();
    TABLE.get_or_init(|| linux::LINUX.iter().map(|&(key, en, es, ca)| (key, (en, es, ca))).collect())
}

/// `key` in a given language. A language missing the row falls back to
/// English, which more people read than Portuguese.
pub fn tr_in(lang: Lang, key: &'static str) -> &'static str {
    let text = base_text(lang, key);
    // On Linux some strings name the desktop's programs, not the Windows
    // ones, and the table of those comes first.
    #[cfg(not(target_os = "windows"))]
    if let Some(row) = linux_table().get(key) {
        return match lang {
            Lang::En => row.0,
            Lang::Es => row.1,
            Lang::Ca => row.2,
            _ => text,
        };
    }
    text
}

/// `key` in a given language, as the main table has it.
fn base_text(lang: Lang, key: &'static str) -> &'static str {
    match lang {
        Lang::Pt => key,
        Lang::En => table().get(key).map_or(key, |row| row.0),
        Lang::Es => table().get(key).map_or(key, |row| row.1),
        Lang::Ca => match catalan_table().get(key) {
            Some(text) => text,
            None => tr_in(Lang::En, key),
        },
        Lang::Ar => match arabic_table().get(key) {
            Some(text) => text,
            None => tr_in(Lang::En, key),
        },
        Lang::Hu => match hungarian_table().get(key) {
            Some(text) => text,
            None => tr_in(Lang::En, key),
        },
        other => match other.extra_column().and_then(|column| extra_table().get(key)?.get(column)) {
            Some(text) => text,
            None => tr_in(Lang::En, key),
        },
    }
}

/// Replaces each `{name}` in `template` with its value. Unlike `format!`
/// this runs on the translated text, whose words — and so placeholders —
/// may come in another order.
pub fn fill(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match values.iter().find(|(key, _)| *key == name) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text in the current language: `t!("Copiar")` → "Copy" in English.
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::tr($key)
    };
}

/// Translated text with values: `tf!("{n} itens", n = count)`. Each name in
/// the text is a `{name}` filled from the arguments, which only need
/// `Display`.
#[macro_export]
macro_rules! tf {
    ($key:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::tr($key),
            &[$((stringify!($name), ($value).to_string())),*],
        )
    };
}

/// Marks a literal as a key without translating it yet, for tables of labels
/// that are passed through [`tr`] where they are drawn. Lets the coverage
/// test find them.
#[macro_export]
macro_rules! k {
    ($key:literal) => {
        $key
    };
}

/// Picks the singular or plural key by `count` and fills `{n}`.
pub fn plural(count: usize, one: &'static str, many: &'static str) -> String {
    fill(tr(if count == 1 { one } else { many }), &[("n", count.to_string())])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};
    use std::path::Path;

    fn placeholders(text: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut rest = text;
        while let Some(start) = rest.find('{') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('}') else { break };
            found.insert(after[..end].to_string());
            rest = &after[end + 1..];
        }
        found
    }

    #[test]
    fn rows_are_unique_and_keep_their_placeholders() {
        let mut seen = HashSet::new();
        for &(pt, en, es) in strings::STRINGS {
            assert!(seen.insert(pt), "duplicated row: {pt:?}");
            assert!(!en.trim().is_empty() && !es.trim().is_empty(), "empty translation for {pt:?}");
            assert_eq!(placeholders(pt), placeholders(en), "placeholders differ in English: {pt:?}");
            assert_eq!(placeholders(pt), placeholders(es), "placeholders differ in Spanish: {pt:?}");
        }
    }

    /// Unescapes what a Rust string literal can hold in this code base.
    fn unescape(literal: &str) -> String {
        let mut out = String::new();
        let mut chars = literal.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            }
        }
        out
    }

    fn keys_in(source: &str, found: &mut Vec<String>) {
        for marker in ["t!(", "tf!(", "k!("] {
            let mut rest = source;
            while let Some(at) = rest.find(marker) {
                // `t!(` must not be the end of `fmt!(`, `assert!(`…
                let before = rest[..at].chars().last();
                let starts_word = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                // The key may start on the next line, as rustfmt puts long ones.
                let after = rest[at + marker.len()..].trim_start();
                let Some(body) = after.strip_prefix('"') else {
                    rest = &rest[at + marker.len()..];
                    continue;
                };
                let mut end = 0;
                let bytes = body.as_bytes();
                while end < bytes.len() && bytes[end] != b'"' {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                if starts_word {
                    found.push(unescape(&body[..end.min(body.len())]));
                }
                rest = &body[end.min(body.len())..];
            }
        }
    }

    fn walk(dir: &Path, found: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && !path.ends_with("i18n.rs")
                && let Ok(source) = std::fs::read_to_string(&path)
            {
                keys_in(&source, found);
            }
        }
    }

    #[test]
    fn every_row_has_every_extra_language_with_its_placeholders() {
        let extra: std::collections::HashMap<&str, &[&str; 6]> =
            extra::EXTRA.iter().map(|(pt, row)| (*pt, row)).collect();
        assert_eq!(extra.len(), extra::EXTRA.len(), "duplicated row in i18n/extra.rs");
        for &(pt, _, _) in strings::STRINGS {
            let row = extra.get(pt).unwrap_or_else(|| panic!("no row in i18n/extra.rs for {pt:?}"));
            for text in row.iter() {
                assert!(!text.trim().is_empty(), "empty translation for {pt:?}");
                assert_eq!(placeholders(pt), placeholders(text), "placeholders differ: {pt:?} -> {text:?}");
            }
        }
        for (pt, _) in extra::EXTRA {
            assert!(strings::STRINGS.iter().any(|row| row.0 == *pt), "row in extra.rs without one in strings.rs: {pt:?}");
        }
    }

    #[test]
    fn every_row_has_arabic_with_its_placeholders() {
        let arabic: std::collections::HashMap<&str, &str> = arabic::ARABIC.iter().copied().collect();
        assert_eq!(arabic.len(), arabic::ARABIC.len(), "duplicated row in i18n/arabic.rs");
        let mut missing = Vec::new();
        for &(pt, _, _) in strings::STRINGS {
            match arabic.get(pt) {
                Some(text) => {
                    assert!(!text.trim().is_empty(), "empty Arabic for {pt:?}");
                    assert_eq!(placeholders(pt), placeholders(text), "placeholders differ: {pt:?} -> {text:?}");
                }
                None => missing.push(pt),
            }
        }
        assert!(missing.is_empty(), "no row in i18n/arabic.rs for:
{missing:#?}");
        for (pt, _) in arabic::ARABIC {
            assert!(strings::STRINGS.iter().any(|row| row.0 == *pt), "row in arabic.rs without one in strings.rs: {pt:?}");
        }
    }

    #[test]
    fn every_row_has_hungarian_with_its_placeholders() {
        let hungarian: std::collections::HashMap<&str, &str> = hungarian::HUNGARIAN.iter().copied().collect();
        assert_eq!(hungarian.len(), hungarian::HUNGARIAN.len(), "duplicated row in i18n/hungarian.rs");
        let mut missing = Vec::new();
        for &(pt, _, _) in strings::STRINGS {
            match hungarian.get(pt) {
                Some(text) => {
                    assert!(!text.trim().is_empty(), "empty Hungarian for {pt:?}");
                    assert_eq!(placeholders(pt), placeholders(text), "placeholders differ: {pt:?} -> {text:?}");
                }
                None => missing.push(pt),
            }
        }
        assert!(missing.is_empty(), "no row in i18n/hungarian.rs for:
{missing:#?}");
        for (pt, _) in hungarian::HUNGARIAN {
            assert!(strings::STRINGS.iter().any(|row| row.0 == *pt), "row in hungarian.rs without one in strings.rs: {pt:?}");
        }
        assert_eq!(tr_in(Lang::Hu, "Copiar"), "Másolás");
        assert_eq!(Lang::parse("hu-HU"), Some(Lang::Hu));
        assert_eq!(Lang::for_country("HU"), Some(Lang::Hu));
    }

    #[test]
    fn every_row_has_catalan_with_its_placeholders() {
        let catalan: std::collections::HashMap<&str, &str> = catalan::CATALAN.iter().copied().collect();
        assert_eq!(catalan.len(), catalan::CATALAN.len(), "duplicated row in i18n/catalan.rs");
        let mut missing = Vec::new();
        for &(pt, _, _) in strings::STRINGS {
            match catalan.get(pt) {
                Some(text) => {
                    assert!(!text.trim().is_empty(), "empty Catalan for {pt:?}");
                    assert_eq!(placeholders(pt), placeholders(text), "placeholders differ: {pt:?} -> {text:?}");
                }
                None => missing.push(pt),
            }
        }
        assert!(missing.is_empty(), "no row in i18n/catalan.rs for:
{missing:#?}");
        for (pt, _) in catalan::CATALAN {
            assert!(strings::STRINGS.iter().any(|row| row.0 == *pt), "row in catalan.rs without one in strings.rs: {pt:?}");
        }
        assert_eq!(tr_in(Lang::Ca, "Copiar"), "Copia");
        assert_eq!(Lang::parse("ca-ES"), Some(Lang::Ca));
        assert_eq!(Lang::for_country("AD"), Some(Lang::Ca));
    }

    #[test]
    fn every_linux_text_has_its_key_and_its_placeholders() {
        let rows: std::collections::HashMap<&str, (&str, &str, &str)> =
            linux::LINUX.iter().map(|&(key, en, es, ca)| (key, (en, es, ca))).collect();
        assert_eq!(rows.len(), linux::LINUX.len(), "duplicated row in i18n/linux.rs");
        for &(pt, _, _) in strings::STRINGS {
            if let Some((en, es, ca)) = rows.get(pt) {
                for text in [en, es, ca] {
                    assert!(!text.trim().is_empty(), "empty Linux text for {pt:?}");
                    assert_eq!(placeholders(pt), placeholders(text), "placeholders differ: {pt:?} -> {text:?}");
                }
            }
        }
        // Every row of the Linux table must be a key of the main table: a
        // key that is not used anywhere would be a text nobody ever sees.
        for (pt, _) in rows.iter() {
            assert!(strings::STRINGS.iter().any(|row| row.0 == *pt), "row in linux.rs without one in strings.rs: {pt:?}");
        }
        assert_eq!(tr_in(Lang::Ca, "Explorador do Windows"), "Gestor de fitxers");
        assert_eq!(tr_in(Lang::Es, "Explorador do Windows"), "Explorador de archivos");
    }

    #[test]
    fn languages_round_trip_through_their_stored_number() {
        for lang in Lang::ALL {
            assert_eq!(Lang::from_u8(lang.as_u8()), lang);
            assert_eq!(Lang::parse(lang.code()), Some(lang));
        }
        assert_eq!(tr_in(Lang::De, "Copiar"), "Kopieren");
        assert_eq!(tr_in(Lang::Ja, "texto que não existe na tabela"), "texto que não existe na tabela");
    }

    #[test]
    fn every_key_in_the_code_has_a_translation() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut found = Vec::new();
        walk(&crates, &mut found);
        assert!(found.len() > 100, "the scan found only {} keys; is it reading the crates?", found.len());
        let rows: HashSet<&str> = strings::STRINGS.iter().map(|row| row.0).collect();
        let mut missing: Vec<&String> = found.iter().filter(|key| !rows.contains(key.as_str())).collect();
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "keys without a row in i18n/strings.rs:\n{missing:#?}");
    }

    #[test]
    fn languages_parse_from_settings_and_locales() {
        assert_eq!(Lang::parse("pt-BR"), Some(Lang::Pt));
        assert_eq!(Lang::parse("pt_PT"), Some(Lang::Pt));
        assert_eq!(Lang::parse("es-MX"), Some(Lang::Es));
        assert_eq!(Lang::parse("EN-us"), Some(Lang::En));
        assert_eq!(Lang::parse("fr-FR"), Some(Lang::Fr));
        assert_eq!(Lang::parse("zh-CN"), Some(Lang::Zh));
        assert_eq!(Lang::parse("ko-KR"), None);
        assert_eq!(Lang::for_country("JP"), Some(Lang::Ja));
        assert_eq!(Lang::for_country("AT"), Some(Lang::De));
        assert_eq!(Lang::parse("auto"), None);
        assert_eq!(Lang::for_country("br"), Some(Lang::Pt));
        assert_eq!(Lang::for_country("AR"), Some(Lang::Es));
        assert_eq!(Lang::for_country("US"), None);
        assert_eq!(Lang::parse("ar-SA"), Some(Lang::Ar));
        assert_eq!(Lang::for_country("EG"), Some(Lang::Ar));
    }

    #[test]
    fn fill_puts_values_in_any_order_and_keeps_unknown_names() {
        let values = [("a", "1".to_string()), ("b", "2".to_string())];
        assert_eq!(fill("{b} antes de {a}", &values), "2 antes de 1");
        assert_eq!(fill("{c} e {", &values), "{c} e {");
    }

    #[test]
    fn missing_rows_fall_back_to_the_key() {
        assert_eq!(tr_in(Lang::En, "texto que não existe na tabela"), "texto que não existe na tabela");
        assert_eq!(tr_in(Lang::En, "Copiar"), "Copy");
        assert_eq!(tr_in(Lang::Es, "Copiar"), "Copiar");
    }
}
