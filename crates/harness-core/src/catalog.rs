//! Plugin catalogs (marketplaces): git repositories that list plugins.
//!
//! A catalog is added once for all projects with `harness marketplace add` and
//! kept in `~/.harness/marketplaces/<name>/`. It describes its plugins in
//! one or both of these files:
//!
//! - `.claude-plugin/marketplace.json`: Claude Code plugins;
//! - `.agents/plugins/marketplace.json`: Codex plugins.
//!
//! ```json
//! {
//!   "name": "claude-plugins-official",
//!   "plugins": [
//!     { "name": "code-review", "description": "...", "source": "./plugins/code-review" },
//!     { "name": "other", "source": { "source": "git-subdir",
//!       "url": "https://github.com/owner/repo.git", "path": "plugins/other",
//!       "ref": "main", "sha": "..." } }
//!   ]
//! }
//! ```
//!
//! This module only reads catalogs. Fetching them with git and copying a
//! plugin into a project is done by `plugin_install`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mcp::is_simple_name;
use crate::plugins::{CLAUDE, CODEX};
use crate::text;

/// Where each agent's catalog file is inside a catalog repository.
pub const CATALOG_FILES: &[(&str, &str)] = &[
    (CLAUDE, ".claude-plugin/marketplace.json"),
    (CODEX, ".agents/plugins/marketplace.json"),
];

/// The list of added catalogs, `~/.harness/marketplaces.toml`.
pub const REGISTRY_FILE: &str = "marketplaces.toml";
/// Their copies, `~/.harness/marketplaces/<name>/`.
pub const CATALOGS_DIR: &str = "marketplaces";

/// Where a plugin's files come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A folder inside the catalog itself, relative to its top folder.
    InCatalog(String),
    /// Another git repository, optionally a folder inside it.
    Git {
        url: String,
        path: Option<String>,
        /// A branch or tag.
        reference: Option<String>,
        /// An exact commit; wins over `reference`.
        sha: Option<String>,
    },
    /// A kind the harness cannot fetch yet, such as npm.
    Unsupported(String),
}

/// One plugin a catalog offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    /// "claude" or "codex": which catalog file listed it.
    pub agent: &'static str,
    pub catalog: String,
    pub description: String,
    pub source: Source,
    /// The entry itself, for plugins without their own `plugin.json`
    /// (`"strict": false`): the harness writes one from it.
    pub fields: serde_json::Map<String, serde_json::Value>,
}

impl Entry {
    /// `"strict": false` means the catalog entry is the plugin's manifest.
    pub fn is_its_own_manifest(&self) -> bool {
        self.fields.get("strict") == Some(&serde_json::Value::Bool(false))
    }

    /// `<catalog>/<plugin>`, as `source` in harness.toml.
    pub fn id(&self) -> String {
        format!("{}/{}", self.catalog, self.name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("{0} has neither .claude-plugin/marketplace.json nor .agents/plugins/marketplace.json; is it a plugin catalog?")]
    NoCatalogFile(String),
    #[error("{path} is not a valid catalog file: {reason}")]
    BadFile { path: String, reason: String },
    #[error(
        "catalog name {0:?} is not allowed; use lowercase letters, digits, '-' and '_' \
         (give another one with --name)"
    )]
    BadName(String),
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// The catalog files found in one catalog folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    /// The name inside the files (the same in both when there are two).
    pub name: String,
    pub entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct RawCatalog {
    name: String,
    #[serde(default)]
    plugins: Vec<serde_json::Map<String, serde_json::Value>>,
}

impl Catalog {
    /// Reads every catalog file in `dir`. `name` overrides the name in the files.
    pub fn read(dir: &Path, name: Option<&str>) -> Result<Self, CatalogError> {
        let mut found_name = None;
        let mut entries = Vec::new();
        let mut any = false;
        for (agent, file) in CATALOG_FILES {
            let path = dir.join(file);
            if !path.is_file() {
                continue;
            }
            any = true;
            let shown = path.display().to_string();
            let text = fs::read_to_string(&path).map_err(|source| CatalogError::Io {
                path: shown.clone(),
                source,
            })?;
            let raw: RawCatalog =
                serde_json::from_str(&text).map_err(|e| CatalogError::BadFile {
                    path: shown.clone(),
                    reason: e.to_string(),
                })?;
            let catalog = name.unwrap_or(&raw.name).to_string();
            found_name.get_or_insert(catalog.clone());
            for fields in raw.plugins {
                if let Some(entry) = entry(agent, &catalog, fields) {
                    entries.push(entry);
                }
            }
        }
        if !any {
            return Err(CatalogError::NoCatalogFile(dir.display().to_string()));
        }
        let name = found_name.unwrap_or_default();
        if !is_simple_name(&name) {
            return Err(CatalogError::BadName(name));
        }
        Ok(Self { name, entries })
    }
}

/// One entry, or `None` for an entry without a usable name.
fn entry(
    agent: &'static str,
    catalog: &str,
    fields: serde_json::Map<String, serde_json::Value>,
) -> Option<Entry> {
    let name = fields.get("name")?.as_str()?.to_string();
    let description = fields
        .get("description")
        .and_then(|d| d.as_str())
        .map(text::safe)
        .unwrap_or_default();
    let source = source(fields.get("source"));
    Some(Entry {
        name,
        agent,
        catalog: catalog.to_string(),
        description,
        source,
        fields,
    })
}

fn source(value: Option<&serde_json::Value>) -> Source {
    let text = |object: &serde_json::Value, key: &str| {
        object.get(key).and_then(|v| v.as_str()).map(str::to_string)
    };
    match value {
        Some(serde_json::Value::String(path)) => Source::InCatalog(path.clone()),
        Some(object @ serde_json::Value::Object(_)) => {
            let kind = text(object, "source").unwrap_or_default();
            let git = |url: Option<String>, path| match url {
                Some(url) => Source::Git {
                    url,
                    path,
                    reference: text(object, "ref"),
                    sha: text(object, "sha"),
                },
                None => Source::Unsupported(kind.clone()),
            };
            match kind.as_str() {
                "local" => match text(object, "path") {
                    Some(path) => Source::InCatalog(path),
                    None => Source::Unsupported(kind),
                },
                "url" | "git" | "git-subdir" => git(text(object, "url"), text(object, "path")),
                "github" => git(
                    text(object, "repo").map(|repo| github_url(&repo)),
                    text(object, "path"),
                ),
                _ => Source::Unsupported(kind),
            }
        }
        _ => Source::Unsupported("none".to_string()),
    }
}

/// `owner/repo` on GitHub as a clone address.
pub fn github_url(repo: &str) -> String {
    format!("https://github.com/{}.git", repo.trim_end_matches(".git"))
}

/// Joins a relative path from a catalog to `base`, refusing anything that
/// leaves it (`..`, an absolute path).
pub fn inside(base: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    let mut path = base.to_path_buf();
    for part in relative.components() {
        match part {
            Component::Normal(name) => path.push(name),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(path)
}

/// How a catalog was added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogConfig {
    /// What was given to `harness marketplace add`: `owner/repo`, a git
    /// address or a local folder.
    pub source: String,
    /// A local folder is read where it is; everything else is cloned.
    #[serde(default)]
    pub local: bool,
}

/// `~/.harness/marketplaces.toml`: the catalogs Lisa added, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub marketplaces: BTreeMap<String, CatalogConfig>,
}

impl Registry {
    /// Reads the list; no file means no catalogs yet.
    pub fn load(harness_home: &Path) -> Result<Self, CatalogError> {
        let path = harness_home.join(REGISTRY_FILE);
        let shown = path.display().to_string();
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| CatalogError::BadFile {
                path: shown,
                reason: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(CatalogError::Io {
                path: shown,
                source,
            }),
        }
    }

    pub fn save(&self, harness_home: &Path) -> Result<(), CatalogError> {
        let path = harness_home.join(REGISTRY_FILE);
        let io = |source| CatalogError::Io {
            path: path.display().to_string(),
            source,
        };
        fs::create_dir_all(harness_home).map_err(io)?;
        let text = toml::to_string(self).expect("the registry is plain TOML");
        fs::write(&path, text).map_err(io)
    }

    /// The folder with a catalog's files.
    pub fn dir(&self, harness_home: &Path, name: &str) -> PathBuf {
        match self.marketplaces.get(name) {
            Some(config) if config.local => PathBuf::from(&config.source),
            _ => harness_home.join(CATALOGS_DIR).join(name),
        }
    }

    /// Every plugin of every catalog. A catalog that cannot be read is
    /// reported, not fatal, so one broken catalog does not hide the others.
    pub fn entries(&self, harness_home: &Path) -> (Vec<Entry>, Vec<CatalogError>) {
        let mut entries = Vec::new();
        let mut errors = Vec::new();
        for name in self.marketplaces.keys() {
            match Catalog::read(&self.dir(harness_home, name), Some(name)) {
                Ok(catalog) => entries.extend(catalog.entries),
                Err(e) => errors.push(e),
            }
        }
        (entries, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, file: &str, text: &str) {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    const CLAUDE_CATALOG: &str = r#"{
        "name": "official",
        "owner": {"name": "Someone"},
        "plugins": [
            {"name": "review", "description": "Reviews code", "source": "./plugins/review"},
            {"name": "remote", "source": {"source": "git-subdir",
                "url": "https://example.com/r.git", "path": "plugins/remote",
                "ref": "v1", "sha": "abc"}},
            {"name": "gh", "source": {"source": "github", "repo": "owner/gh"}},
            {"name": "pkg", "source": {"source": "npm", "package": "x"}},
            {"name": "lsp", "source": "./plugins/lsp", "strict": false,
             "lspServers": {"x": {}}},
            {"description": "no name"}
        ]
    }"#;

    const CODEX_CATALOG: &str = r#"{
        "name": "official",
        "plugins": [
            {"name": "review", "source": {"source": "local", "path": "./plugins/review"}}
        ]
    }"#;

    #[test]
    fn reads_both_agents_catalog_files() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".claude-plugin/marketplace.json",
            CLAUDE_CATALOG,
        );
        write(
            dir.path(),
            ".agents/plugins/marketplace.json",
            CODEX_CATALOG,
        );
        let catalog = Catalog::read(dir.path(), None).unwrap();
        assert_eq!(catalog.name, "official");
        let find = |agent, name| {
            catalog
                .entries
                .iter()
                .find(|e| e.agent == agent && e.name == name)
                .unwrap()
                .clone()
        };
        let review = find(CLAUDE, "review");
        assert_eq!(review.description, "Reviews code");
        assert_eq!(review.source, Source::InCatalog("./plugins/review".into()));
        assert_eq!(review.id(), "official/review");
        assert_eq!(
            find(CLAUDE, "remote").source,
            Source::Git {
                url: "https://example.com/r.git".into(),
                path: Some("plugins/remote".into()),
                reference: Some("v1".into()),
                sha: Some("abc".into()),
            }
        );
        assert!(matches!(
            find(CLAUDE, "gh").source,
            Source::Git { ref url, .. } if url == "https://github.com/owner/gh.git"
        ));
        assert_eq!(
            find(CLAUDE, "pkg").source,
            Source::Unsupported("npm".into())
        );
        assert!(find(CLAUDE, "lsp").is_its_own_manifest());
        assert!(!review.is_its_own_manifest());
        assert_eq!(
            find(CODEX, "review").source,
            Source::InCatalog("./plugins/review".into())
        );
        assert_eq!(
            catalog.entries.len(),
            6,
            "the entry without a name is skipped"
        );
    }

    #[test]
    fn a_folder_without_catalog_files_or_with_a_bad_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Catalog::read(dir.path(), None),
            Err(CatalogError::NoCatalogFile(_))
        ));
        write(
            dir.path(),
            ".claude-plugin/marketplace.json",
            r#"{"name": "Bad Name", "plugins": []}"#,
        );
        assert!(matches!(
            Catalog::read(dir.path(), None),
            Err(CatalogError::BadName(_))
        ));
        assert_eq!(
            Catalog::read(dir.path(), Some("mine")).unwrap().name,
            "mine"
        );
    }

    #[test]
    fn catalog_paths_cannot_leave_their_folder() {
        let base = Path::new("/cat");
        assert_eq!(
            inside(base, "./plugins/x"),
            Some(PathBuf::from("/cat/plugins/x"))
        );
        for bad in ["../x", "/etc", "a/../../b"] {
            assert_eq!(inside(base, bad), None, "{bad}");
        }
    }

    #[test]
    fn the_registry_is_saved_and_read_back() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(Registry::load(home.path()).unwrap(), Registry::default());
        let mut registry = Registry::default();
        registry.marketplaces.insert(
            "official".into(),
            CatalogConfig {
                source: "anthropics/claude-plugins-official".into(),
                local: false,
            },
        );
        registry.marketplaces.insert(
            "mine".into(),
            CatalogConfig {
                source: "/home/lisa/catalog".into(),
                local: true,
            },
        );
        registry.save(home.path()).unwrap();
        let loaded = Registry::load(home.path()).unwrap();
        assert_eq!(loaded, registry);
        assert_eq!(
            loaded.dir(home.path(), "official"),
            home.path().join("marketplaces/official")
        );
        assert_eq!(
            loaded.dir(home.path(), "mine"),
            PathBuf::from("/home/lisa/catalog")
        );
    }
}
