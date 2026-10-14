//! The empty files Claude Code makes when it starts, and how the harness
//! tells them apart from an agent's real changes.
//!
//! With `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB` (which the harness sets) Claude
//! Code on Linux runs commands in a bubblewrap sandbox. Before that it
//! creates empty files where a package manager or git would look for
//! settings (`package.json`, `.npmrc`, `.env`, ...), so the sandbox can make
//! them read-only, and adds them to `.git/info/exclude` so `git status`
//! does not show them. Checked against Claude Code 2.1.283 and 2.1.299.
//!
//! The tripwire puts `info/exclude` back after every role. When the only
//! change was this block, it does not stop the task, and the harness removes
//! the new empty stub files so they do not end up in the role's commit. Any
//! other change to `info/exclude` still stops the task. A block left from a
//! run the harness did not watch (Lisa's own Claude Code, the retrospective)
//! is removed before the next role, so it cannot hide that role's files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::git::Repo;

/// The files Claude Code creates empty at the top of the project.
pub const STUB_FILES: &[&str] = &[
    "bunfig.toml",
    "package.json",
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".gitmodules",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    ".env",
    ".env.local",
    ".env.development",
    ".env.development.local",
    ".env.test",
    ".env.test.local",
    ".env.production",
    ".env.production.local",
];

/// The comment line Claude Code writes above its lines in `info/exclude`.
const EXCLUDE_HEADER: &str = "# claude-code scrub-mode stubs";

/// Is the change from `before` to `after` only Claude Code's block of stub
/// lines added at the end of `info/exclude`?
///
/// Every added line must be empty, the header, or `/<stub>` for a name in
/// [`STUB_FILES`]. A line like `/src` or `*` would hide real work from
/// `git status`, so it does not pass.
pub fn only_stub_lines(before: &[u8], after: &[u8]) -> bool {
    let Some(added) = after.strip_prefix(before) else {
        return false;
    };
    let Ok(added) = std::str::from_utf8(added) else {
        return false;
    };
    let mut has_stub = false;
    for line in added.lines() {
        if line.is_empty() || line == EXCLUDE_HEADER {
            continue;
        }
        let is_stub = line
            .strip_prefix('/')
            .is_some_and(|name| STUB_FILES.contains(&name));
        if !is_stub {
            return false;
        }
        has_stub = true;
    }
    has_stub
}

/// Removes Claude Code's blocks from the text of `info/exclude` and returns
/// the new text with the stub names the blocks listed, or `None` when there
/// is no block. The blank lines just above a block go with it.
fn strip_stub_blocks(text: &str) -> Option<(String, Vec<String>)> {
    if !text.lines().any(|line| line == EXCLUDE_HEADER) {
        return None;
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut names = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        if line == EXCLUDE_HEADER {
            in_block = true;
            while kept.last().is_some_and(|last| last.is_empty()) {
                kept.pop();
            }
            continue;
        }
        if in_block {
            let stub = line
                .strip_prefix('/')
                .filter(|name| STUB_FILES.contains(name));
            if let Some(name) = stub {
                names.push(name.to_string());
                continue;
            }
            in_block = false;
        }
        kept.push(line);
    }
    let mut new_text = kept.join("\n");
    if !new_text.is_empty() {
        new_text.push('\n');
    }
    Some((new_text, names))
}

/// Removes what an earlier unwatched run of Claude Code left: its block in
/// `exclude` and the empty, untracked stub files that block hid. Without
/// this, a role could put a real `package.json` or `.npmrc` there and git
/// would not show it, so it would skip every check and review.
///
/// # Errors
/// Reading or writing `exclude`, or removing a stub file, failed.
pub fn clear_leftovers(repo: &Repo, exclude: &Path) -> io::Result<()> {
    let text = match fs::read_to_string(exclude) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let Some((new_text, names)) = strip_stub_blocks(&text) else {
        return Ok(());
    };
    fs::write(exclude, new_text)?;
    for name in names {
        let path = repo.root().join(&name);
        if is_empty_file(&path) && !repo.is_tracked(Path::new(&name)) {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Is `path` a plain file (not a link) with nothing in it?
fn is_empty_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() == 0)
}

/// The stub files that were missing in the project before a role.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingStubs {
    paths: Vec<PathBuf>,
}

impl MissingStubs {
    /// Notes which of [`STUB_FILES`] are missing in `root` now.
    pub fn note(root: &Path) -> Self {
        let paths = STUB_FILES
            .iter()
            .map(|name| root.join(name))
            // `symlink_metadata` also sees a link that points nowhere. Only
            // "not found" counts: a file we cannot look at is not missing.
            .filter(|path| {
                fs::symlink_metadata(path).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
            })
            .collect();
        Self { paths }
    }

    /// Removes the noted files that now exist as empty plain files, and
    /// returns them. A file with content or a link is the agent's own work
    /// and is left for the usual checks.
    pub fn remove_new_empty(&self) -> io::Result<Vec<PathBuf>> {
        let mut removed = Vec::new();
        for path in &self.paths {
            if is_empty_file(path) {
                fs::remove_file(path)?;
                removed.push(path.clone());
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Claude Code appends to `info/exclude` (its own format).
    fn claude_block() -> String {
        let lines: Vec<String> = STUB_FILES.iter().map(|name| format!("/{name}")).collect();
        format!("\n\n{EXCLUDE_HEADER}\n{}", lines.join("\n"))
    }

    #[test]
    fn claude_codes_block_is_only_stub_lines() {
        let before = b"# git ls-files --others --exclude-from=.git/info/exclude\n";
        let mut after = before.to_vec();
        after.extend_from_slice(claude_block().as_bytes());

        assert!(only_stub_lines(before, &after));
    }

    #[test]
    fn a_line_that_hides_other_files_is_not_a_stub_line() {
        let before = b"# excludes\n";
        for added in [
            "\n/src\n",
            "\n*\n",
            "\n/package.json\n/.git/hooks\n",
            "\npackage.json\n",
        ] {
            let after = [before.as_slice(), added.as_bytes()].concat();
            assert!(!only_stub_lines(before, &after), "{added:?}");
        }
    }

    #[test]
    fn a_changed_old_line_or_nothing_added_is_not_a_stub_change() {
        let before = b"# excludes\n/build\n";
        let changed = format!("# excludes\n{}", claude_block());
        assert!(!only_stub_lines(before, changed.as_bytes()));
        let header_only = format!("# excludes\n/build\n\n{EXCLUDE_HEADER}\n");
        assert!(!only_stub_lines(before, header_only.as_bytes()));
    }

    #[test]
    fn new_empty_stubs_are_removed_and_the_rest_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".env"), "").unwrap();
        let missing = MissingStubs::note(root);
        // What Claude Code and an agent do during the role.
        fs::write(root.join("package.json"), "").unwrap();
        fs::write(root.join("yarn.lock"), "").unwrap();
        fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: 9\n").unwrap();
        fs::write(root.join("notes.txt"), "").unwrap();

        let removed = missing.remove_new_empty().unwrap();

        assert_eq!(removed, [root.join("package.json"), root.join("yarn.lock")]);
        assert!(root.join(".env").exists(), "it was there before the role");
        assert!(root.join("pnpm-lock.yaml").exists(), "it has content");
        assert!(root.join("notes.txt").exists(), "not a stub name");
    }

    #[test]
    fn leftovers_of_an_unwatched_run_are_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let root = dir.path();
        fs::write(root.join(".gitmodules"), "").unwrap();
        repo.commit_all("an empty .gitmodules of the project")
            .unwrap();
        let exclude = repo.git_common_dir().unwrap().join("info/exclude");
        let old = fs::read_to_string(&exclude).unwrap_or_default();
        fs::write(
            &exclude,
            format!("{old}\n\n{EXCLUDE_HEADER}\n/package.json\n/.npmrc\n/.gitmodules"),
        )
        .unwrap();
        fs::write(root.join("package.json"), "").unwrap();
        fs::write(root.join(".npmrc"), "registry=https://example.com/\n").unwrap();

        clear_leftovers(&repo, &exclude).unwrap();

        let new = fs::read_to_string(&exclude).unwrap();
        assert!(!new.contains(EXCLUDE_HEADER), "{new}");
        assert!(!new.contains("/package.json"), "{new}");
        assert!(!root.join("package.json").exists());
        assert!(
            root.join(".npmrc").exists(),
            "it has content: git shows it now"
        );
        assert!(root.join(".gitmodules").exists(), "it is tracked");
    }

    #[test]
    fn stripping_keeps_lisas_own_lines() {
        let text = format!("# excludes\n/build\n\n\n{EXCLUDE_HEADER}\n/.env\n/yarn.lock\n/notes\n");

        let (new_text, names) = strip_stub_blocks(&text).unwrap();

        assert_eq!(new_text, "# excludes\n/build\n/notes\n");
        assert_eq!(names, [".env", "yarn.lock"]);
        assert!(strip_stub_blocks("# excludes\n/build\n").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_with_a_stub_name_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let missing = MissingStubs::note(root);
        fs::write(root.join("empty"), "").unwrap();
        std::os::unix::fs::symlink(root.join("empty"), root.join(".npmrc")).unwrap();

        assert!(missing.remove_new_empty().unwrap().is_empty());
        fs::symlink_metadata(root.join(".npmrc")).unwrap();
    }
}
