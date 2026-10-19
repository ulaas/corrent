//! Translations. The English text is the key (gettext style), so untranslated strings simply
//! show in English. Catalogs live in the repository's `assets/i18n/<lang>.toml` as `"English" = "Translation"`.
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
        Lang::Turkish => Some(TR.get_or_init(|| parse(include_str!("../../../assets/i18n/tr.toml")))),
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
        $crate::tr($s)
    };
    ($s:literal, $($k:ident = $v:expr),+ $(,)?) => {
        $crate::tr_args($s, &[$((stringify!($k), &$v as &dyn std::fmt::Display)),+])
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
        let cat = parse(include_str!("../../../assets/i18n/tr.toml"));
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
            r#"(RichText::new|\.label|\.button|\.small_button|\.on_hover_text|\.on_disabled_hover_text|\.hint_text|\.labeled|\.menu_button|\.selected_text|primary_button\(ui, p,|secondary_button\(ui,|danger_button\(ui, p,|heading\(ui, p,|ToastKind::[A-Za-z]+,|busy\.insert\([a-z_]+,|busy: Some\()\(?\s*"[^"]*[A-Za-z]{2,}[^"]*""#,
        )
        .unwrap();
        let icon_label = regex::Regex::new(r#"format!\("\{\}\s+(\{\}\s+)?[A-Za-z]{2,}"#).unwrap();
        let prose = regex::Regex::new(r#"format!\(\s*"[^"]*[A-Za-z]{3,} [A-Za-z]{2,}"#).unwrap();
        let open_paren = regex::Regex::new(r#"\(\s*\n\s*""#).unwrap();
        let continuation = regex::Regex::new(r"\\\n\s*").unwrap();
        let mut found = Vec::new();
        let root = workspace_root();
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        // GUI crate folders and files with user-visible text (paths that moved to crates/ are skipped).
        for dir in ["src/app", "src/ui", "src/engine", "src/rss"] {
            files.extend(std::fs::read_dir(root.join(dir)).into_iter().flatten().flatten().map(|e| e.path()));
        }
        files.extend(["src/util.rs", "src/config.rs", "src/store.rs", "src/backup.rs"].map(|f| root.join(f)));
        files.retain(|p| p.exists());
        // Library crates: everything but this one (and the libmpv bindings, which have no UI text).
        for krate in crate_dirs(&root) {
            files.extend(rust_files(&krate.join("src")));
        }
        {
            for p in files {
                if p.extension().is_none_or(|x| x != "rs") || p.ends_with("ui_tests.rs") || p.ends_with("e2e_tests.rs")
                {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                // Skip inline test modules (`#[cfg(test)]` + `mod x {`), not `mod x;` declarations.
                let text = text.replace("\r\n", "\n");
                let code = match text.find("#[cfg(test)]\nmod tests {") {
                    Some(i) => &text[..i],
                    None => text.as_str(),
                };
                // `call(\n    "literal"` → `call("literal"`, so multi-line calls are checked too.
                let joined = open_paren.replace_all(code, "(\"");
                // "text \ ⏎   more" string continuations: one line, so the closing quote is found.
                let joined = continuation.replace_all(&joined, "");
                for (n, line) in joined.lines().enumerate() {
                    // Sentences built with format! (the file header comment of exported themes is fine).
                    let sentence = prose.is_match(line) && !line.contains("# Corrent theme");
                    // URLs given as examples aren't text to translate.
                    let url_only = line.contains("(\"http://") || line.contains("(\"https://");
                    if (calls.is_match(line) || icon_label.is_match(line) || sentence) && !url_only {
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

    fn workspace_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
    }

    /// `crates/*` except this crate and the libmpv bindings.
    fn crate_dirs(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut v: Vec<_> = std::fs::read_dir(root.join("crates"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("src").is_dir() && !p.ends_with("corrent-i18n") && !p.ends_with("corrent-mpv"))
            .collect();
        v.sort();
        v
    }

    /// `.rs` files under `dir`, recursively.
    fn rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        out
    }

    /// All `t!("…")` literals in the GUI crate and the library crates.
    pub(crate) fn source_keys() -> Vec<String> {
        let mut out = std::collections::BTreeSet::new();
        let root = workspace_root();
        let mut files = rust_files(&root.join("src"));
        for krate in crate_dirs(&root) {
            files.extend(rust_files(&krate.join("src")));
        }
        {
            for p in files {
                {
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
