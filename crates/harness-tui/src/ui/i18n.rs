//! The TUI in several languages. Every text on the screen comes from a
//! translation file, `locales/<code>.toml`:
//!
//! ```toml
//! language = "EN"          # shown on the language button
//! [tabs]
//! tasks = "Tasks"          # the text for the key "tabs.tasks"
//! ```
//!
//! English and Russian are built into the program. More languages (or changed
//! texts for these two) are read from `~/.harness/locales/*.toml`. A text
//! missing from a translation is shown in English. The chosen language is
//! saved in `~/.harness/tui.toml`.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::fs;
use std::path::Path;

/// The built-in translations; English first, it is the default.
const BUILT_IN: [(&str, &str); 2] = [
    ("en", include_str!("../../locales/en.toml")),
    ("ru", include_str!("../../locales/ru.toml")),
];
pub const DEFAULT: &str = "en";
/// `<home>/locales/<code>.toml` holds more languages.
pub const LOCALES_DIR: &str = "locales";
/// `<home>/tui.toml` remembers the language: `language = "ru"`.
pub const SETTINGS_FILE: &str = "tui.toml";

#[derive(Debug, Clone)]
pub struct Language {
    /// `en`, `ru`: the file name.
    pub code: String,
    /// What the language button shows.
    pub label: String,
    texts: BTreeMap<String, String>,
}

impl Language {
    /// Reads a translation file into flat keys: `[tabs] tasks` -> `tabs.tasks`.
    fn parse(code: &str, text: &str) -> Result<Self, String> {
        let table: toml::Table = toml::from_str(text).map_err(|e| format!("{code}.toml: {e}"))?;
        let mut texts = BTreeMap::new();
        flatten("", &table, &mut texts);
        let label = texts
            .remove("language")
            .unwrap_or_else(|| code.to_uppercase());
        Ok(Self {
            code: code.to_string(),
            label,
            texts,
        })
    }
}

fn flatten(prefix: &str, table: &toml::Table, out: &mut BTreeMap<String, String>) {
    for (key, value) in table {
        let key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(inner) => flatten(&key, inner, out),
            toml::Value::String(text) => {
                out.insert(key, text.clone());
            }
            other => {
                out.insert(key, other.to_string());
            }
        }
    }
}

/// All languages the TUI knows and the one in use.
#[derive(Debug, Clone)]
pub struct I18n {
    languages: Vec<Language>,
    current: usize,
    /// Translation files that could not be read.
    pub problems: Vec<String>,
}

impl I18n {
    /// The built-in languages, plus the files in `<home>/locales/`, starting
    /// with the language saved in `<home>/tui.toml`.
    pub fn load(home: Option<&Path>) -> Self {
        let mut i18n = Self {
            languages: Vec::new(),
            current: 0,
            problems: Vec::new(),
        };
        for (code, text) in BUILT_IN {
            match Language::parse(code, text) {
                Ok(language) => i18n.languages.push(language),
                Err(problem) => i18n.problems.push(problem),
            }
        }
        if let Some(home) = home {
            i18n.load_dir(&home.join(LOCALES_DIR));
            if let Some(code) = saved_language(home) {
                i18n.select(&code);
            }
        }
        i18n
    }

    fn load_dir(&mut self, dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();
        for path in files {
            let Some(code) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            let language = fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", path.display()))
                .and_then(|text| Language::parse(&code, &text));
            match language {
                // A file for a built-in language changes its texts.
                Ok(language) => match self.languages.iter_mut().find(|l| l.code == code) {
                    Some(known) => {
                        known.label = language.label;
                        known.texts.extend(language.texts);
                    }
                    None => self.languages.push(language),
                },
                Err(problem) => self.problems.push(problem),
            }
        }
    }

    pub fn code(&self) -> &str {
        self.languages
            .get(self.current)
            .map_or(DEFAULT, |l| l.code.as_str())
    }

    pub fn label(&self) -> &str {
        self.languages
            .get(self.current)
            .map_or("EN", |l| l.label.as_str())
    }

    /// Switches to `code`; an unknown code changes nothing.
    pub fn select(&mut self, code: &str) -> bool {
        match self.languages.iter().position(|l| l.code == code) {
            Some(index) => {
                self.current = index;
                true
            }
            None => false,
        }
    }

    /// The next language, round the list.
    pub fn next(&mut self) {
        if !self.languages.is_empty() {
            self.current = (self.current + 1) % self.languages.len();
        }
    }

    /// Remembers the language for the next start.
    pub fn save(&self, home: &Path) -> Result<(), String> {
        save_setting(home, "language", self.code())
    }

    /// The text for `key`, in English if the language lacks it, else the key.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        let find = |index: usize| {
            self.languages
                .get(index)
                .and_then(|l| l.texts.get(key))
                .map(String::as_str)
        };
        find(self.current)
            .or_else(|| {
                let english = self.languages.iter().position(|l| l.code == DEFAULT)?;
                find(english)
            })
            .unwrap_or(key)
    }

    /// The text for `key` with `{name}` replaced by the values given.
    pub fn f(&self, key: &str, values: &[(&str, &dyn Display)]) -> String {
        let mut text = self.t(key).to_string();
        for (name, value) in values {
            text = text.replace(&format!("{{{name}}}"), &value.to_string());
        }
        text
    }
}

fn saved_language(home: &Path) -> Option<String> {
    saved_setting(home, "language")
}

/// A setting of the TUI kept in `tui.toml`, such as `language` or `theme`.
pub fn saved_setting(home: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(home.join(SETTINGS_FILE)).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get(key)?.as_str().map(str::to_string)
}

/// Saves one setting in `tui.toml`, keeping the others.
pub fn save_setting(home: &Path, key: &str, value: &str) -> Result<(), String> {
    let path = home.join(SETTINGS_FILE);
    let mut table: toml::Table = fs::read_to_string(&path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default();
    table.insert(key.to_string(), toml::Value::String(value.to_string()));
    let text = toml::to_string(&table).map_err(|e| e.to_string())?;
    fs::create_dir_all(home)
        .and_then(|()| fs::write(&path, text))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(code: &str) -> Vec<String> {
        let (_, text) = BUILT_IN.iter().find(|(c, _)| *c == code).unwrap();
        Language::parse(code, text)
            .unwrap()
            .texts
            .into_keys()
            .collect()
    }

    #[test]
    fn every_built_in_language_has_every_text() {
        let english = keys("en");
        assert!(english.len() > 50);
        for (code, _) in BUILT_IN {
            assert_eq!(keys(code), english, "{code}.toml differs from en.toml");
        }
    }

    #[test]
    fn english_is_the_default_and_the_choice_is_saved() {
        let home = tempfile::tempdir().unwrap();
        let mut i18n = I18n::load(Some(home.path()));
        assert!(i18n.problems.is_empty(), "{:?}", i18n.problems);
        assert_eq!((i18n.code(), i18n.label()), ("en", "EN"));
        assert_eq!(i18n.t("tabs.tasks"), "Tasks");
        assert_eq!(
            i18n.f("projects.opened", &[("name", &"demo")]),
            "Opened project demo"
        );

        i18n.next();
        assert_eq!((i18n.code(), i18n.t("tabs.tasks")), ("ru", "Задачи"));
        i18n.save(home.path()).unwrap();
        assert_eq!(I18n::load(Some(home.path())).code(), "ru");

        i18n.next();
        assert_eq!(i18n.code(), "en");
        assert_eq!(i18n.t("no.such.key"), "no.such.key");
    }

    #[test]
    fn more_languages_come_from_files() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(LOCALES_DIR);
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("de.toml"),
            "language = \"DE\"\n[tabs]\ntasks = \"Aufgaben\"\n",
        )
        .unwrap();
        fs::write(dir.join("ru.toml"), "[tabs]\nretro = \"Итоги\"\n").unwrap();
        fs::write(dir.join("xx.toml"), "not toml [").unwrap();

        let mut i18n = I18n::load(Some(home.path()));
        assert_eq!(i18n.problems.len(), 1, "{:?}", i18n.problems);
        assert!(i18n.select("de"));
        assert_eq!(i18n.label(), "DE");
        assert_eq!(i18n.t("tabs.tasks"), "Aufgaben");
        // Not translated: English.
        assert_eq!(i18n.t("tabs.roles"), "Roles");

        assert!(i18n.select("ru"));
        assert_eq!(i18n.t("tabs.retro"), "Итоги");
        assert_eq!(i18n.t("tabs.tasks"), "Задачи");
        assert!(!i18n.select("fr"));
    }
}
