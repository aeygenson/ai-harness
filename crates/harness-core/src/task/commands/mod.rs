//! Which shell commands the roles may run, shared by every agent's adapter:
//! commands no role may run, commands a reading role may not run, the checks
//! a checking role (Security) may run, and the user's own extra checks
//! (`extra_commands` in `harness.toml`). The programs an extra command may
//! never start with are in `never`.
//!
//! A command here is a prefix of whole words: `git diff` matches
//! `git diff HEAD~1`, but not `git difftool`.

mod never;

/// Commands no role may run: the harness makes the commits, and only it
/// sends work to GitHub (`gh` uses the user's GitHub login).
pub const DENIED_FOR_ALL: &[&str] = &["git commit", "git push", "gh"];

/// Commands a role without [`Commands::Any`](crate::task::permissions::Commands)
/// may not run: they change files, the git state, install or publish packages,
/// or reach the network. Reading, running tests and audit tools stay allowed.
/// The list cannot be complete; the git check after the role still catches
/// any changed file.
pub const DENIED_FOR_READERS: &[&str] = &[
    "rm",
    "mv",
    "cp",
    "git add",
    "git checkout",
    "git reset",
    "git restore",
    "git stash",
    "git clean",
    "git rm",
    "git mv",
    "git apply",
    "git switch",
    "git merge",
    "git rebase",
    "git branch",
    "git tag",
    "git update-ref",
    "git worktree",
    "git pull",
    "git fetch",
    "git bisect",
    // `git config` can make git itself run a command later (`core.fsmonitor`).
    "git config",
    "cargo build",
    "cargo install",
    "cargo run",
    "cargo publish",
    "npm install",
    "npm publish",
    "pip install",
    "go install",
    "dotnet build",
    "curl",
    "wget",
];

/// The commands a checking role (Security) may run in agents that allow
/// commands one by one (Claude Code): reading the change with git, and the
/// vulnerability scanners of common ecosystems. A scanner that is not
/// installed simply fails. The user adds more with `extra_commands`.
pub const CHECK_COMMANDS: &[&str] = &[
    "git diff",
    "git log",
    "git show",
    "cargo audit",
    "cargo deny",
    "npm audit",
    "pnpm audit",
    "yarn audit",
    "pip-audit",
    "govulncheck",
    "bundle audit",
    "dotnet list package",
    "osv-scanner",
    "semgrep",
    "gitleaks",
];

/// The most words an extra command may have: a program and two more words,
/// such as `osv-scanner scan source`.
const MAX_WORDS: usize = 3;

/// Endings of programs on Windows: `git.exe` runs the same program as `git`.
const WINDOWS_ENDINGS: &[&str] = &[".exe", ".cmd", ".bat", ".com", ".ps1"];

/// Checks one of the user's `extra_commands` and says what is wrong with it.
///
/// It must be one to three plain words (letters, digits, `-`, `_`, `.`)
/// separated by single spaces, must not start with a program from a list of
/// programs that run any code, install or change files (letter case and a
/// Windows ending such as `.exe` do not matter), and must not overlap with a
/// command no role or no reading role may run. This is a best effort: the
/// lists cannot name every such program.
pub fn check_extra_command(command: &str) -> Result<(), String> {
    let words: Vec<&str> = command.split(' ').collect();
    if command.trim().is_empty() {
        return Err("an extra command cannot be empty".into());
    }
    if words.len() > MAX_WORDS {
        return Err(format!(
            "extra command {command:?} is too long; write only the program and up to \
             {} more words, such as \"trivy fs\"",
            MAX_WORDS - 1
        ));
    }
    if let Some(word) = words.iter().find(|word| !plain_word(word)) {
        return Err(format!(
            "extra command {command:?} has {word:?}, which is not a plain word; \
             use only letters, digits, '-', '_' and '.', with one space between words"
        ));
    }
    let program = program_name(words[0]);
    if never::is_never_extra(&program) {
        return Err(format!(
            "extra command {command:?} is refused: {program} can run any code, \
             install or change files"
        ));
    }
    let named = std::iter::once(program.as_str())
        .chain(words[1..].iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    let denied = DENIED_FOR_ALL.iter().chain(DENIED_FOR_READERS);
    if let Some(denied) = denied.into_iter().find(|denied| overlaps(&named, denied)) {
        return Err(format!(
            "extra command {command:?} is refused: it would allow {denied:?}, \
             which a checking role may not run"
        ));
    }
    Ok(())
}

/// Letters, digits, `-`, `_` and `.`, not starting with `-` or `.`, so a word
/// can be neither an option nor a hidden path.
fn plain_word(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with(['-', '.'])
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The program as the system finds it: lower case (macOS and Windows do not
/// tell `Git` from `git`) and without a Windows ending (`git.exe` is `git`).
fn program_name(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    WINDOWS_ENDINGS
        .iter()
        .find_map(|ending| lower.strip_suffix(ending))
        .map_or(lower.clone(), str::to_string)
}

/// Does either command start with the other, word by word?
fn overlaps(a: &str, b: &str) -> bool {
    starts_with_words(a, b) || starts_with_words(b, a)
}

/// Do the words of `command` begin with all the words of `prefix`?
fn starts_with_words(command: &str, prefix: &str) -> bool {
    let mut words = command.split_whitespace();
    prefix
        .split_whitespace()
        .all(|expected| words.next() == Some(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_scanner_commands_pass() {
        assert_eq!(check_extra_command("trivy"), Ok(()));
        assert_eq!(check_extra_command("trivy fs"), Ok(()));
        assert_eq!(check_extra_command("osv-scanner scan source"), Ok(()));
        assert_eq!(check_extra_command("ghost"), Ok(()));
    }

    #[test]
    fn empty_long_or_odd_commands_are_refused() {
        for odd in [
            "",
            "   ",
            "a b c d",
            "trivy;rm",
            "trivy *",
            "trivy --fix",
            "./scan",
            " trivy",
            "trivy  fs",
            "trivy\tfs",
            "trivy\nrm",
        ] {
            assert!(check_extra_command(odd).is_err(), "{odd:?} passed");
        }
    }

    #[test]
    fn programs_that_run_code_install_or_write_are_refused_in_any_spelling() {
        for broad in [
            "bash",
            "python -m",
            "python3.12",
            "perl5.34",
            "lua5.4",
            "tclsh8.6",
            "npx audit-ci",
            "sudo trivy",
            "awk",
            "sed",
            "timeout",
            "docker run",
            "pnpm dlx",
            "npm i",
            "cargo test",
            "tee",
            "source",
            "poetry run",
            "aws s3",
            "Bash",
            "bash.exe",
            "PowerShell.EXE",
            "npm.cmd",
            "git-lfs install",
        ] {
            assert!(check_extra_command(broad).is_err(), "{broad:?} passed");
        }
    }

    #[test]
    fn git_and_commands_overlapping_a_denied_one_are_refused() {
        for denied in [
            "git",
            "git config",
            "git blame",
            "Git.exe push",
            "gh api",
            "GH",
            "rm",
            "curl",
            "wget",
        ] {
            assert!(check_extra_command(denied).is_err(), "{denied:?} passed");
        }
    }
}
