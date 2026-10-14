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
//! An empty file holds no command and no setting, so the harness removes the
//! new empty ones after the role, and does not stop the task for the exclude
//! lines that hide exactly these files. Any other change to `info/exclude`
//! still stops it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
            // `symlink_metadata` also sees a link that points nowhere.
            .filter(|path| fs::symlink_metadata(path).is_err())
            .collect();
        Self { paths }
    }

    /// Removes the noted files that now exist as empty plain files, and
    /// returns them. A file with content or a link is the agent's own work
    /// and is left for the usual checks.
    pub fn remove_new_empty(&self) -> io::Result<Vec<PathBuf>> {
        let mut removed = Vec::new();
        for path in &self.paths {
            let Ok(meta) = fs::symlink_metadata(path) else {
                continue;
            };
            if meta.is_file() && meta.len() == 0 {
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
