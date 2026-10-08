//! Translations. The English text is the key (gettext style), so untranslated strings simply
//! show in English. Catalogs live in `assets/i18n/<lang>.toml` as `"English" = "Translation"`.
//!
//! Placeholders are named: `t!("Created “{name}”", name = x)`. Translations must use the same
//! names (checked by a test).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    English = 0,
    Turkish = 1,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::English, Lang::Turkish];

    pub fn code(self) -> &'static str {
        match self {
            Lang::English => "en",
            Lang::Turkish => "tr",
        }
    }

    /// The language's own name (shown untranslated in the picker).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::Turkish => "Türkçe",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        let primary = code.split(['-', '_']).next()?.to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| l.code() == primary)
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// `setting`: "" = follow the system, otherwise a language code.
/// The OS language if Corrent has it, else English.
pub fn system_language() -> Lang {
    sys_locale::get_locale().and_then(|l| Lang::from_code(&l)).unwrap_or(Lang::English)
}

pub fn set_language(setting: &str) {
    let lang =
        if setting.trim().is_empty() { system_language() } else { Lang::from_code(setting).unwrap_or(Lang::English) };
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Turkish,
        _ => Lang::English,
    }
}

fn catalog(lang: Lang) -> Option<&'static HashMap<String, &'static str>> {
    static TR: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    match lang {
        Lang::English => None,
        Lang::Turkish => Some(TR.get_or_init(|| parse(include_str!("../assets/i18n/tr.toml")))),
    }
}

fn parse(text: &str) -> HashMap<String, &'static str> {
    let table: toml::Table = text.parse().unwrap_or_else(|e| {
        tracing::error!("translation catalog is invalid: {e}");
        Default::default()
    });
    table
        .into_iter()
        .filter_map(|(k, v)| match v {
            toml::Value::String(s) if !s.is_empty() => Some((k, &*Box::leak(s.into_boxed_str()))),
            _ => None,
        })
        .collect()
}

/// Translate in a given language (tests use this; the UI uses [`tr`]).
pub fn tr_in(lang: Lang, s: &'static str) -> &'static str {
    catalog(lang).and_then(|c| c.get(s).copied()).unwrap_or(s)
}

pub fn tr(s: &'static str) -> &'static str {
    tr_in(current(), s)
}

/// Translate and fill `{name}` placeholders.
pub fn tr_args(s: &'static str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = tr(s).to_string();
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), &v.to_string());
    }
    out
}

/// `t!("text")` → `&'static str`; `t!("Hello {name}", name = x)` → `String`.
#[macro_export]
macro_rules! t {
    ($s:literal) => {
        $crate::i18n::tr($s)
    };
    ($s:literal, $($k:ident = $v:expr),+ $(,)?) => {
        $crate::i18n::tr_args($s, &[$((stringify!($k), &$v as &dyn std::fmt::Display)),+])
    };
}

/// Marks a key in a `const`/`static` (where `t!` can't run); translate at use with [`tr`].
#[macro_export]
macro_rules! tk {
    ($s:literal) => {
        $s
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_and_fallback() {
        assert_eq!(Lang::from_code("tr-TR"), Some(Lang::Turkish));
        assert_eq!(Lang::from_code("en_US"), Some(Lang::English));
        assert_eq!(Lang::from_code("de"), None);
        assert_eq!(tr_in(Lang::English, "Pause"), "Pause");
        assert_eq!(tr_in(Lang::Turkish, "a string nobody translated"), "a string nobody translated");
    }

    /// Every `t!("…")` in the source has a Turkish translation with the same placeholders.
    #[test]
    fn turkish_catalog_is_complete() {
        let cat = parse(include_str!("../assets/i18n/tr.toml"));
        let mut missing = Vec::new();
        let mut bad = Vec::new();
        for key in source_keys() {
            match cat.get(&key) {
                None => missing.push(key),
                Some(t) => {
                    if placeholders(&key) != placeholders(t) {
                        bad.push(format!("{key} → {t}"));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{} untranslated:\n{}", missing.len(), missing.join("\n"));
        assert!(bad.is_empty(), "placeholders differ:\n{}", bad.join("\n"));
    }

    /// CI gate: user-visible text in the UI goes through `t!`. Flags plain literals passed to
    /// the usual text-taking calls in src/app and src/ui (outside tests).
    #[test]
    fn no_hard_coded_ui_strings() {
        let calls = regex::Regex::new(
            r#"(RichText::new|\.label|\.button|\.small_button|\.on_hover_text|\.on_disabled_hover_text|\.hint_text|\.labeled|\.menu_button|\.selected_text|primary_button\(ui, p,|secondary_button\(ui,|danger_button\(ui, p,|heading\(ui, p,|ToastKind::[A-Za-z]+,)\(?\s*"[^"]*[A-Za-z]{2,}[^"]*""#,
        )
        .unwrap();
        let icon_label = regex::Regex::new(r#"format!\("\{\}\s+(\{\}\s+)?[A-Za-z]{2,}"#).unwrap();
        let mut found = Vec::new();
        for dir in ["src/app", "src/ui"] {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.extension().is_none_or(|x| x != "rs") || p.ends_with("ui_tests.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                // Skip inline test modules (`#[cfg(test)]` + `mod x {`), not `mod x;` declarations.
                let code = match text.find(
                    "#[cfg(test)]
mod tests {",
                ) {
                    Some(i) => &text[..i],
                    None => text.as_str(),
                };
                for (n, line) in code.lines().enumerate() {
                    if calls.is_match(line) || icon_label.is_match(line) {
                        found.push(format!("{}:{}: {}", p.display(), n + 1, line.trim()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "wrap these in t!():\n{}", found.join("\n"));
    }

    fn placeholders(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s
            .split('{')
            .skip(1)
            .filter_map(|p| p.split_once('}').map(|(n, _)| n.to_string()))
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_'))
            .collect();
        v.sort();
        v
    }

    /// All `t!("…")` literals in src/.
    pub(crate) fn source_keys() -> Vec<String> {
        let mut out = std::collections::BTreeSet::new();
        let mut stack = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("i18n.rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    for marker in ["t!(", "tk!("] {
                        for (i, _) in text.match_indices(marker) {
                            let prev = text[..i].chars().next_back();
                            if prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                                continue; // e.g. assert!(…) is not t!
                            }
                            // The literal may start on the next line.
                            let rest = text[i + marker.len()..].trim_start();
                            if rest.starts_with('"')
                                && let Some(lit) = read_literal(rest)
                            {
                                out.insert(lit);
                            }
                        }
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// The Rust string literal at the start of `s` (which begins with `"`), unescaped.
    fn read_literal(s: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = s.chars().skip(1);
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '\n' => {
                        // line continuation: skip leading whitespace
                        let rest: String = chars.by_ref().collect();
                        let trimmed = rest.trim_start();
                        let mut more = String::from("\"");
                        more.push_str(trimmed);
                        return read_literal(&more).map(|m| out + &m);
                    }
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
        None
    }
}
