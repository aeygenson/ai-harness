//! The empty files Claude Code makes when it starts, and how the harness
//! tells them apart from an agent's real changes.
//!
//! With `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB` (which the harness sets) Claude
//! Code on Linux runs commands in a bubblewrap sandbox. Before that it
//! creates empty files where a package manager or git would look for
//! settings (`package.json`, `.npmrc`, `.env`, ...), so the sandbox can make
//! them read-only, and appends them to `.git/info/exclude` so `git status`
//! does not show them. Checked against Claude Code 2.1.283 and 2.1.299.
//!
//! The tripwire puts `info/exclude` back after every role. When the only
//! change was this block, it does not stop the task, and the harness removes
//! the new empty stub files so they do not end up in the role's commit. Any
//! other change to `info/exclude` still stops the task. A block left by a run
//! the harness did not watch (Lisa's own Claude Code, the retrospective) is
//! removed before git status is trusted again, so it cannot hide files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::git::{GitError, Repo};

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
const EXCLUDE_HEADER: &[u8] = b"# claude-code scrub-mode stubs";

/// The lines of `bytes`, without the `\r` of Windows line ends.
fn lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
}

/// Is `line` `/<name>` for a name in [`STUB_FILES`]?
fn is_stub_line(line: &[u8]) -> bool {
    line.strip_prefix(b"/")
        .is_some_and(|name| STUB_FILES.iter().any(|stub| stub.as_bytes() == name))
}

/// Does the text of `info/exclude` hold Claude Code's header line?
pub fn has_stub_block(exclude: &[u8]) -> bool {
    lines(exclude).any(|line| line == EXCLUDE_HEADER)
}

/// Is the change from `before` to `after` exactly one block of Claude Code's
/// stub lines added at the end of `info/exclude`?
///
/// The added text must start on a new line, hold the header once, and
/// otherwise only empty lines and `/<stub>` lines for names in
/// [`STUB_FILES`]. A line like `/src` or `*` would hide real work from
/// `git status`, so it does not pass.
pub fn only_stub_lines(before: &[u8], after: &[u8]) -> bool {
    let Some(added) = after.strip_prefix(before) else {
        return false;
    };
    // Glued to `before`'s last line, the first added "line" would change it.
    let ends_with_line = before.is_empty() || before.ends_with(b"\n");
    if !ends_with_line && !added.starts_with(b"\n") {
        return false;
    }
    let mut headers = 0;
    let mut stubs = 0;
    for line in lines(added) {
        if line.is_empty() {
            continue;
        }
        if line == EXCLUDE_HEADER {
            headers += 1;
        } else if is_stub_line(line) {
            stubs += 1;
        } else {
            return false;
        }
    }
    headers == 1 && stubs > 0
}

/// Removes Claude Code's blocks from the text of `info/exclude`: the header,
/// the blank lines just above it and every `/...` line after it. Unknown
/// names go too: keeping one would leave it hiding files for good, while
/// dropping it only makes git show a file. Returns `None` without a block.
fn strip_stub_blocks(text: &[u8]) -> Option<Vec<u8>> {
    if !has_stub_block(text) {
        return None;
    }
    let mut kept: Vec<&[u8]> = Vec::new();
    let mut in_block = false;
    // `split` keeps the `\r` here, so Windows line ends survive.
    for line in text.split(|&b| b == b'\n') {
        let bare = line.strip_suffix(b"\r").unwrap_or(line);
        if bare == EXCLUDE_HEADER {
            in_block = true;
            while kept.last().is_some_and(|last| last.trim_ascii().is_empty()) {
                kept.pop();
            }
            continue;
        }
        if in_block && bare.starts_with(b"/") {
            continue;
        }
        in_block = false;
        kept.push(line);
    }
    let mut new_text = kept.join(&b'\n');
    if !new_text.is_empty() && !new_text.ends_with(b"\n") {
        new_text.push(b'\n');
    }
    Some(new_text)
}

/// Removes what an earlier unwatched run of Claude Code left: its block in
/// `info/exclude`, then the stub files that are now visible to git and empty.
/// A stub with content stays, so git status shows it and Lisa checks it.
///
/// # Errors
/// Git failed, or reading or writing `info/exclude` or a stub file failed.
pub fn clear_leftovers(repo: &Repo) -> Result<(), GitError> {
    let path = repo.git_common_dir()?.join("info").join("exclude");
    let io_error = |source| GitError::Io {
        path: path.clone(),
        source,
    };
    let text = match fs::read(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_error(e)),
    };
    let Some(new_text) = strip_stub_blocks(&text) else {
        return Ok(());
    };
    // Removed first, so writing never follows a link put in its place.
    fs::remove_file(&path).map_err(io_error)?;
    fs::write(&path, new_text).map_err(io_error)?;
    let all: Vec<PathBuf> = STUB_FILES
        .iter()
        .map(|name| repo.root().join(name))
        .collect();
    remove_visible_empty(repo, &all)
}

/// Removes those of `paths` that git shows as new (not tracked, not ignored)
/// and that are empty plain files. A tracked or ignored file is Lisa's.
fn remove_visible_empty(repo: &Repo, paths: &[PathBuf]) -> Result<(), GitError> {
    let changed = repo.changed_files()?;
    for path in paths {
        let Ok(relative) = path.strip_prefix(repo.root()) else {
            continue;
        };
        let shown = relative
            .to_str()
            .is_some_and(|name| changed.iter().any(|c| c == name));
        if shown && is_empty_file(path) && !repo.is_tracked(relative) {
            fs::remove_file(path).map_err(|source| GitError::Io {
                path: path.clone(),
                source,
            })?;
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

    /// Removes the noted files that git now shows as new and that are empty
    /// plain files. Call it only after `info/exclude` is put back, so git
    /// sees them. A file with content or a link is the agent's own work.
    ///
    /// # Errors
    /// Git failed, or a file could not be removed.
    pub fn remove_new_empty(&self, repo: &Repo) -> Result<(), GitError> {
        remove_visible_empty(repo, &self.paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> String {
        String::from_utf8_lossy(EXCLUDE_HEADER).into_owned()
    }

    /// What Claude Code appends to `info/exclude` (its own format).
    fn claude_block() -> String {
        let lines: Vec<String> = STUB_FILES.iter().map(|name| format!("/{name}")).collect();
        format!("\n\n{}\n{}", header(), lines.join("\n"))
    }

    #[test]
    fn claude_codes_block_is_only_stub_lines() {
        let before = b"# git ls-files --others --exclude-from=.git/info/exclude\n";
        let after = [before.as_slice(), claude_block().as_bytes()].concat();

        assert!(only_stub_lines(before, &after));
        assert!(only_stub_lines(b"", claude_block().as_bytes()));
    }

    #[test]
    fn a_line_that_hides_other_files_is_not_a_stub_line() {
        let before = b"# excludes\n";
        let h = header();
        for added in [
            format!("\n{h}\n/src\n"),
            format!("\n{h}\n*\n"),
            format!("\n{h}\n/package.json\n/.git/hooks\n"),
            format!("\n{h}\npackage.json\n"),
            "\n/.env\n".to_string(),
            format!("\n{h}\n/.env\n{h}\n/.npmrc"),
        ] {
            let after = [before.as_slice(), added.as_bytes()].concat();
            assert!(!only_stub_lines(before, &after), "{added:?}");
        }
    }

    #[test]
    fn a_changed_old_line_is_not_a_stub_change() {
        let before = b"# excludes\n/build\n";
        let changed = format!("# excludes\n{}", claude_block());
        assert!(!only_stub_lines(before, changed.as_bytes()));
        // Glued to a last line without a line end, it reads `/build/.env`.
        let glued = format!("/build/.env\n{}\n/.npmrc", header());
        assert!(!only_stub_lines(b"/build", glued.as_bytes()));
    }

    #[test]
    fn stripping_keeps_lisas_lines_and_drops_unknown_block_lines() {
        let text = format!(
            "# excludes\r\n/build\r\n\r\n\n{}\n/.env\n/deno.json\nnotes\n",
            header()
        );

        let new_text = strip_stub_blocks(text.as_bytes()).unwrap();

        assert_eq!(new_text, b"# excludes\r\n/build\r\nnotes\n");
        assert!(strip_stub_blocks(b"# excludes\n/build\n").is_none());
    }

    #[test]
    fn leftovers_of_an_unwatched_run_are_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let root = dir.path();
        fs::write(root.join(".gitmodules"), "").unwrap();
        fs::write(root.join(".gitignore"), "/.env.local\n").unwrap();
        repo.commit_all("the project's own files").unwrap();
        let exclude = repo.git_common_dir().unwrap().join("info/exclude");
        let old = fs::read_to_string(&exclude).unwrap_or_default();
        fs::write(&exclude, format!("{old}{}", claude_block())).unwrap();
        fs::write(root.join("package.json"), "").unwrap();
        fs::write(root.join(".env.local"), "").unwrap();
        fs::write(root.join(".npmrc"), "registry=https://example.com/\n").unwrap();

        clear_leftovers(&repo).unwrap();

        assert_eq!(fs::read_to_string(&exclude).unwrap(), old);
        assert!(!root.join("package.json").exists());
        assert!(
            root.join(".npmrc").exists(),
            "it has content: git shows it now"
        );
        assert!(root.join(".gitmodules").exists(), "it is tracked");
        assert!(
            root.join(".env.local").exists(),
            "Lisa's .gitignore hides it"
        );
        assert_eq!(repo.changed_files().unwrap(), [".npmrc"]);
    }

    #[test]
    fn new_empty_stubs_are_removed_and_the_rest_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let root = dir.path();
        fs::write(root.join(".env"), "").unwrap();
        let missing = MissingStubs::note(root);
        // What Claude Code and an agent do during the role.
        fs::write(root.join("package.json"), "").unwrap();
        fs::write(root.join("yarn.lock"), "").unwrap();
        fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: 9\n").unwrap();
        fs::write(root.join("notes.txt"), "").unwrap();

        missing.remove_new_empty(&repo).unwrap();

        assert!(!root.join("package.json").exists());
        assert!(!root.join("yarn.lock").exists());
        assert!(root.join(".env").exists(), "it was there before the role");
        assert!(root.join("pnpm-lock.yaml").exists(), "it has content");
        assert!(root.join("notes.txt").exists(), "not a stub name");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_with_a_stub_name_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let root = dir.path();
        let missing = MissingStubs::note(root);
        fs::write(root.join("empty"), "").unwrap();
        std::os::unix::fs::symlink(root.join("empty"), root.join(".npmrc")).unwrap();

        missing.remove_new_empty(&repo).unwrap();

        fs::symlink_metadata(root.join(".npmrc")).unwrap();
    }
}
