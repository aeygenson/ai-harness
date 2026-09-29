//! Copies a plugin from a catalog into a project (`harness plugin add` and
//! `harness plugin update`).
//!
//! 1. `fetch`: find the plugin's files. A folder inside the catalog is used
//!    where it is; a plugin in another git repository is downloaded first.
//! 2. `stage`: copy them into a new folder next to the project's plugins and
//!    look at what the plugin brings. A plugin whose catalog entry is its
//!    manifest (`"strict": false`) gets a `plugin.json` written from it.
//! 3. `put_in_place`: swap the new folder in, and `changes` tells what changed.
//!
//! Nothing here runs the plugin. The checks before a role runs (`plugins`)
//! stay the same for plugins that came from a catalog.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::catalog::{inside, Entry, Source};
use crate::git::{self, GitError};
use crate::plugins::{self, manifest, Contents, PluginError, CLAUDE, CODEX};

/// Catalog fields that describe the listing, not the plugin.
const LISTING_ONLY: &[&str] = &["source", "strict", "category", "tags"];

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("plugin {name:?} comes from {kind:?}, which the harness cannot fetch yet")]
    Unsupported { name: String, kind: String },
    #[error("plugin {name:?}: its path {path:?} leaves its repository")]
    BadPath { name: String, path: String },
    #[error("plugin {name:?}: {path} is not a folder")]
    NotAFolder { name: String, path: String },
    #[error("plugin {name:?}: cannot download it: {source}")]
    Fetch {
        name: String,
        #[source]
        source: GitError,
    },
    #[error(transparent)]
    Plugin(#[from] PluginError),
    #[error(
        "plugin {0:?} has apps (ChatGPT connectors), which reach services outside the \
         project; the harness does not support them"
    )]
    Apps(String),
    #[error("cannot copy the plugin: {0}")]
    Io(#[from] io::Error),
}

/// The plugin's files, ready to be copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub dir: PathBuf,
    /// The git commit they are from, if known.
    pub commit: Option<String>,
}

/// Finds the plugin's files. `catalog_dir` is the catalog's folder;
/// `download_dir` is where a plugin from another repository is kept (it is
/// reused on the next update).
pub fn fetch(
    entry: &Entry,
    catalog_dir: &Path,
    download_dir: &Path,
) -> Result<Fetched, InstallError> {
    let bad_path = |path: &str| InstallError::BadPath {
        name: entry.name.clone(),
        path: path.to_string(),
    };
    let (dir, commit) = match &entry.source {
        Source::InCatalog(path) => (
            inside(catalog_dir, path).ok_or_else(|| bad_path(path))?,
            git::head_commit(catalog_dir),
        ),
        Source::Git {
            url,
            path,
            reference,
            sha,
        } => {
            let revision = sha.as_deref().or(reference.as_deref());
            let commit =
                git::fetch(url, revision, download_dir).map_err(|source| InstallError::Fetch {
                    name: entry.name.clone(),
                    source,
                })?;
            let dir = match path {
                Some(path) => inside(download_dir, path).ok_or_else(|| bad_path(path))?,
                None => download_dir.to_path_buf(),
            };
            (dir, Some(commit))
        }
        Source::Unsupported(kind) => {
            return Err(InstallError::Unsupported {
                name: entry.name.clone(),
                kind: kind.clone(),
            })
        }
    };
    if !dir.is_dir() {
        return Err(InstallError::NotAFolder {
            name: entry.name.clone(),
            path: dir.display().to_string(),
        });
    }
    Ok(Fetched { dir, commit })
}

/// Copies the fetched files into `into` (which must not exist yet) and checks
/// them. On error `into` is removed again.
pub fn stage(entry: &Entry, fetched: &Fetched, into: &Path) -> Result<Contents, InstallError> {
    let result = stage_inner(entry, fetched, into);
    if result.is_err() {
        let _ = fs::remove_dir_all(into);
    }
    result
}

fn stage_inner(entry: &Entry, fetched: &Fetched, into: &Path) -> Result<Contents, InstallError> {
    plugins::copy_dir(&fetched.dir, into)?;
    let manifest_path = into.join(manifest(entry.agent));
    if !manifest_path.exists() && entry.agent == CLAUDE && entry.is_its_own_manifest() {
        let fields: serde_json::Map<_, _> = entry
            .fields
            .iter()
            .filter(|(key, _)| !LISTING_ONLY.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        fs::create_dir_all(manifest_path.parent().expect("the manifest is in a folder"))?;
        let text = serde_json::to_string_pretty(&fields).expect("JSON from JSON");
        fs::write(&manifest_path, text + "\n")?;
    }
    let contents = plugins::inspect(into, &entry.name, entry.agent)?;
    if entry.agent == CODEX && contents.apps {
        return Err(InstallError::Apps(entry.name.clone()));
    }
    Ok(contents)
}

/// Replaces the plugin folder `target` with the staged folder `staged`.
pub fn put_in_place(staged: &Path, target: &Path) -> io::Result<()> {
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(staged, target)
}

/// Files that differ between two versions of a plugin folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Compares the files of `old` and `new` (paths relative to each folder).
pub fn changes(old: &Path, new: &Path) -> io::Result<Changes> {
    let old_files = files(old)?;
    let new_files = files(new)?;
    let mut result = Changes::default();
    for (path, bytes) in &new_files {
        match old_files.get(path) {
            None => result.added.push(path.clone()),
            Some(old_bytes) if old_bytes != bytes => result.changed.push(path.clone()),
            Some(_) => {}
        }
    }
    result.removed = old_files
        .keys()
        .filter(|path| !new_files.contains_key(*path))
        .cloned()
        .collect();
    Ok(result)
}

fn files(dir: &Path) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let mut found = BTreeMap::new();
    if dir.exists() {
        collect(dir, dir, &mut found)?;
    }
    Ok(found)
}

fn collect(root: &Path, dir: &Path, found: &mut BTreeMap<String, Vec<u8>>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect(root, &path, found)?;
        } else {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            found.insert(relative.to_string_lossy().into_owned(), fs::read(&path)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::git::Repo;

    fn write(dir: &Path, file: &str, text: &str) {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A catalog repository with a plugin inside and one entry pointing to
    /// another repository.
    fn catalog(remote_url: &str) -> (tempfile::TempDir, Catalog) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            r#"{{"name": "cat", "plugins": [
                {{"name": "review", "source": "./plugins/review"}},
                {{"name": "lsp", "source": "./plugins/lsp", "strict": false,
                  "description": "LSP", "category": "dev", "lspServers": {{"x": {{}}}}}},
                {{"name": "far", "source": {{"source": "git-subdir", "url": "{remote_url}",
                  "path": "plugins/far"}}}},
                {{"name": "escape", "source": "../outside"}},
                {{"name": "npm", "source": {{"source": "npm", "package": "x"}}}}
            ]}}"#
        );
        write(dir.path(), ".claude-plugin/marketplace.json", &text);
        write(
            dir.path(),
            "plugins/review/.claude-plugin/plugin.json",
            r#"{"name": "review"}"#,
        );
        write(dir.path(), "plugins/review/skills/a/SKILL.md", "one");
        write(dir.path(), "plugins/lsp/README.md", "lsp");
        let repo = Repo::init(dir.path()).unwrap();
        repo.commit_all("catalog").unwrap();
        let catalog = Catalog::read(dir.path(), None).unwrap();
        (dir, catalog)
    }

    fn find<'a>(catalog: &'a Catalog, name: &str) -> &'a Entry {
        catalog.entries.iter().find(|e| e.name == name).unwrap()
    }

    #[test]
    fn a_plugin_inside_the_catalog_is_staged_and_checked() {
        let (dir, catalog) = catalog("unused");
        let work = tempfile::tempdir().unwrap();
        let entry = find(&catalog, "review");
        let fetched = fetch(entry, dir.path(), &work.path().join("dl")).unwrap();
        assert_eq!(fetched.dir, dir.path().join("plugins/review"));
        assert_eq!(fetched.commit, git::head_commit(dir.path()));

        let staged = work.path().join("staged");
        let contents = stage(entry, &fetched, &staged).unwrap();
        assert_eq!(contents, Contents::default());
        assert_eq!(
            fs::read_to_string(staged.join("skills/a/SKILL.md")).unwrap(),
            "one"
        );
    }

    #[test]
    fn an_entry_that_is_its_own_manifest_gets_a_plugin_json() {
        let (dir, catalog) = catalog("unused");
        let work = tempfile::tempdir().unwrap();
        let entry = find(&catalog, "lsp");
        let fetched = fetch(entry, dir.path(), &work.path().join("dl")).unwrap();
        let staged = work.path().join("staged");
        let contents = stage(entry, &fetched, &staged).unwrap();
        assert!(contents.servers);
        let manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(staged.join(".claude-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["name"], "lsp");
        assert!(manifest.get("source").is_none() && manifest.get("category").is_none());
    }

    #[test]
    fn a_plugin_from_another_repository_is_downloaded() {
        let remote = tempfile::tempdir().unwrap();
        write(
            remote.path(),
            "plugins/far/.claude-plugin/plugin.json",
            r#"{"name": "far"}"#,
        );
        let remote_repo = Repo::init(remote.path()).unwrap();
        remote_repo.commit_all("far").unwrap();
        let url = format!("file://{}", remote.path().display());
        let (dir, catalog) = catalog(&url);
        let work = tempfile::tempdir().unwrap();
        let fetched = fetch(find(&catalog, "far"), dir.path(), &work.path().join("dl")).unwrap();
        assert_eq!(fetched.dir, work.path().join("dl/plugins/far"));
        assert_eq!(fetched.commit, remote_repo.head().unwrap());
    }

    #[test]
    fn unsupported_or_escaping_sources_are_refused() {
        let (dir, catalog) = catalog("unused");
        let work = tempfile::tempdir().unwrap();
        assert!(matches!(
            fetch(find(&catalog, "escape"), dir.path(), work.path()),
            Err(InstallError::BadPath { .. })
        ));
        assert!(matches!(
            fetch(find(&catalog, "npm"), dir.path(), work.path()),
            Err(InstallError::Unsupported { .. })
        ));
    }

    #[test]
    fn a_codex_plugin_with_apps_is_not_staged() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".agents/plugins/marketplace.json",
            r#"{"name": "cat", "plugins": [{"name": "app", "source": "./p"}]}"#,
        );
        write(
            dir.path(),
            "p/.codex-plugin/plugin.json",
            r#"{"name": "app"}"#,
        );
        write(dir.path(), "p/.app.json", "{}");
        let catalog = Catalog::read(dir.path(), None).unwrap();
        let entry = &catalog.entries[0];
        let work = tempfile::tempdir().unwrap();
        let fetched = fetch(entry, dir.path(), work.path()).unwrap();
        let staged = work.path().join("staged");
        assert!(matches!(
            stage(entry, &fetched, &staged),
            Err(InstallError::Apps(_))
        ));
        assert!(!staged.exists());
    }

    #[test]
    fn changes_between_versions_are_listed_and_the_new_one_replaces_the_old() {
        let work = tempfile::tempdir().unwrap();
        let old = work.path().join("old");
        let new = work.path().join("new");
        write(&old, "same.md", "x");
        write(&old, "edited.md", "1");
        write(&old, "gone.md", "bye");
        write(&new, "same.md", "x");
        write(&new, "edited.md", "2");
        write(&new, "dir/added.md", "hi");
        let found = changes(&old, &new).unwrap();
        assert_eq!(found.added, ["dir/added.md"]);
        assert_eq!(found.changed, ["edited.md"]);
        assert_eq!(found.removed, ["gone.md"]);
        assert!(changes(&new, &new).unwrap().is_empty());

        put_in_place(&new, &old).unwrap();
        assert!(!new.exists());
        assert_eq!(fs::read_to_string(old.join("edited.md")).unwrap(), "2");
        assert!(!old.join("gone.md").exists());
    }
}
