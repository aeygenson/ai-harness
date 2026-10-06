//! `~/.harness/marketplaces.toml`: the list of catalogs Lisa added.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Catalog, CatalogError, Entry, CATALOGS_DIR, REGISTRY_FILE};

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
    /// The `[marketplaces.<name>]` tables: each catalog's name and how it was added.
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

    /// Writes the list to `marketplaces.toml`, creating `harness_home` if needed.
    ///
    /// # Panics
    ///
    /// Only on a bug: the list holds plain strings, which TOML can always write.
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
