//! Adding, updating and removing catalogs. They live only in `~/.harness`, so
//! nothing here touches a project.

use std::fs;
use std::path::Path;

use super::{io, OpsError, ADDING_DIR, DOWNLOADS_DIR};
use crate::git;
use crate::plugins::catalog::{self, Catalog, CatalogConfig, Entry, Registry};

/// `owner/repo` means GitHub; anything that looks like a git address is used as it is.
pub fn clone_url(source: &str) -> Result<String, OpsError> {
    if source.contains("://") || source.starts_with("git@") || source.ends_with(".git") {
        return Ok(source.to_string());
    }
    let parts: Vec<&str> = source.split('/').collect();
    let simple = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if parts.len() == 2 && parts.iter().all(|part| simple(part)) {
        return Ok(catalog::github_url(source));
    }
    Err(OpsError::BadSource(source.to_string()))
}

/// One added catalog, as the list of catalogs shows it.
#[derive(Debug)]
pub struct CatalogInfo {
    pub name: String,
    pub config: CatalogConfig,
    /// The commit of its copy; none for a local folder.
    pub commit: Option<String>,
    /// Its plugins, or why it cannot be read.
    pub entries: Result<Vec<Entry>, String>,
}

/// The added catalogs.
pub fn catalogs(home: &Path) -> Result<Vec<CatalogInfo>, OpsError> {
    let registry = Registry::load(home)?;
    Ok(registry
        .marketplaces
        .iter()
        .map(|(name, config)| {
            let dir = registry.dir(home, name);
            CatalogInfo {
                name: name.clone(),
                config: config.clone(),
                commit: if config.local {
                    None
                } else {
                    git::head_commit(&dir)
                },
                entries: Catalog::read(&dir, Some(name))
                    .map(|c| c.entries)
                    .map_err(|e| e.to_string()),
            }
        })
        .collect())
}

/// Adds a catalog: a local folder (read where it is), `owner/repo` on
/// GitHub or a git address (cloned). `name` replaces the catalog's own name.
pub fn add_catalog(home: &Path, source: &str, name: Option<&str>) -> Result<Catalog, OpsError> {
    let source = source.trim();
    let mut registry = Registry::load(home)?;
    let catalogs = home.join(catalog::CATALOGS_DIR);
    let local = Path::new(source);
    let (config, catalog) = if local.is_dir() {
        let path = local.canonicalize().map_err(io(local))?;
        let catalog = Catalog::read(&path, name)?;
        let config = CatalogConfig {
            source: path.display().to_string(),
            local: true,
        };
        (config, catalog)
    } else {
        let url = clone_url(source)?;
        let adding = catalogs.join(ADDING_DIR);
        let _ = fs::remove_dir_all(&adding);
        git::fetch(&url, None, &adding).map_err(|source| OpsError::Download {
            what: format!("the catalog {url}"),
            source,
        })?;
        let catalog = match Catalog::read(&adding, name) {
            Ok(catalog) => catalog,
            Err(e) => {
                let _ = fs::remove_dir_all(&adding);
                return Err(e.into());
            }
        };
        if registry.marketplaces.contains_key(&catalog.name) {
            let _ = fs::remove_dir_all(&adding);
        } else {
            let target = catalogs.join(&catalog.name);
            let _ = fs::remove_dir_all(&target);
            fs::rename(&adding, &target).map_err(io(&target))?;
        }
        let config = CatalogConfig {
            source: url,
            local: false,
        };
        (config, catalog)
    };
    if registry.marketplaces.contains_key(&catalog.name) {
        return Err(OpsError::CatalogExists(catalog.name));
    }
    registry.marketplaces.insert(catalog.name.clone(), config);
    registry.save(home)?;
    Ok(catalog)
}

/// What updating a catalog did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogUpdate {
    /// A local folder is always read as it is.
    Local,
    /// Already at this commit.
    Same(String),
    /// Now at this commit.
    Updated(String),
}

/// Downloads the newest version of a catalog. Plugins already in projects
/// do not change.
pub fn update_catalog(home: &Path, name: &str) -> Result<CatalogUpdate, OpsError> {
    let registry = Registry::load(home)?;
    let config = registry
        .marketplaces
        .get(name)
        .ok_or_else(|| OpsError::NoCatalog(name.to_string()))?;
    if config.local {
        return Ok(CatalogUpdate::Local);
    }
    let dir = registry.dir(home, name);
    let before = git::head_commit(&dir);
    let after = git::fetch(&config.source, None, &dir).map_err(|source| OpsError::Download {
        what: format!("catalog {name}"),
        source,
    })?;
    Ok(if before.as_deref() == Some(after.as_str()) {
        CatalogUpdate::Same(after)
    } else {
        CatalogUpdate::Updated(after)
    })
}

/// Takes a catalog off the list and deletes its copy and downloads.
/// Plugins already in projects stay there.
pub fn remove_catalog(home: &Path, name: &str) -> Result<(), OpsError> {
    let mut registry = Registry::load(home)?;
    let config = registry
        .marketplaces
        .remove(name)
        .ok_or_else(|| OpsError::NoCatalog(name.to_string()))?;
    let catalogs = home.join(catalog::CATALOGS_DIR);
    if !config.local {
        let _ = fs::remove_dir_all(catalogs.join(name));
    }
    let _ = fs::remove_dir_all(catalogs.join(DOWNLOADS_DIR).join(name));
    registry.save(home)?;
    Ok(())
}
