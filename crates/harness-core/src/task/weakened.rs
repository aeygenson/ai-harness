//! Signs in a step's diff that the role made the project's checks weaker
//! to get to green: a test skipped or deleted, assertions taken out, a
//! linter or analyser silenced, a stub left where code should be.
//!
//! An agent that hits a failing check usually takes the cheapest road to a
//! pass instead of fixing the cause. The role skills ask it not to, but only
//! the diff shows whether it did. The harness reads the diff of the step's
//! own commit, so no agent can hide such a change. A sign is not proof:
//! skipping a test can be right. The next role gets the signs as facts and
//! decides (see [`crate::task::facts`]).
//!
//! The patterns cover several languages, because the harness works on
//! projects of any kind. They are plain text matches, not a parser.

use std::fmt;
use std::path::Path;

use crate::task::permissions;

/// Added text that skips or disables a test.
const SKIP_PATTERNS: &[&str] = &[
    "#[ignore",          // Rust
    ".skip(",            // JavaScript: it.skip, describe.skip, test.skip
    "xit(",              // JavaScript (Jasmine, Mocha)
    "xdescribe(",        // JavaScript (Jasmine, Mocha)
    "@pytest.mark.skip", // Python
    "@unittest.skip",    // Python
    "t.Skip(",           // Go
    "@Disabled",         // Java (JUnit 5)
    "@Ignore",           // Java (JUnit 4)
    "(Skip =",           // C# (xUnit: [Fact(Skip = "...")])
];

/// Added text that switches a linter, type checker or analyser off for some code.
const SILENCE_PATTERNS: &[&str] = &[
    "#[allow(",                // Rust
    "#![allow(",               // Rust, for a whole module
    "#[expect(",               // Rust
    "eslint-disable",          // JavaScript
    "@ts-ignore",              // TypeScript
    "@ts-nocheck",             // TypeScript
    "@ts-expect-error",        // TypeScript
    "# noqa",                  // Python (flake8, ruff)
    "# type: ignore",          // Python (mypy)
    "pylint: disable",         // Python
    "//nolint",                // Go
    "@SuppressWarnings",       // Java
    "#pragma warning disable", // C#
    "NOSONAR",                 // SonarQube
    "nosemgrep",               // Semgrep
    "gitleaks:allow",          // gitleaks: hides a secret finding
    "istanbul ignore",         // JavaScript coverage
    "c8 ignore",               // JavaScript coverage
    "pragma: no cover",        // Python coverage
    "rubocop:disable",         // Ruby
];

/// Added text that leaves work unfinished: a stub or an error swallowed in silence.
const UNFINISHED_PATTERNS: &[&str] = &[
    "todo!(",                  // Rust
    "unimplemented!(",         // Rust
    "NotImplementedError",     // Python
    "NotImplementedException", // C#
    "not implemented",         // many languages, in an error message
    "Not implemented",
    "catch {}", // JavaScript, TypeScript: an empty catch
    "catch (_) {}",
    "except: pass", // Python
];

/// Files whose names end like this are documents, not code: a design or a
/// skill that *mentions* `.skip(` is not a skipped test.
const DOCUMENT_ENDINGS: &[&str] = &[".md", ".markdown", ".txt", ".rst", ".adoc"];

/// At most this many signs are listed, so a huge diff does not fill the prompt.
pub const MAX_SIGNS_SHOWN: usize = 20;

/// What kind of weakening a sign points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A test was skipped or disabled.
    TestSkipped,
    /// A test file was deleted (and not moved: no file of that name was added).
    TestFileDeleted,
    /// More assertion lines were removed than added, over the whole step.
    FewerAssertions,
    /// A linter, type checker or analyser was switched off for some code.
    CheckSilenced,
    /// A stub or an empty error handler was added.
    Unfinished,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Kind::TestSkipped => "test skipped",
            Kind::TestFileDeleted => "test file deleted",
            Kind::FewerAssertions => "assertions removed",
            Kind::CheckSilenced => "check silenced",
            Kind::Unfinished => "unfinished code",
        };
        f.write_str(text)
    }
}

/// One sign: its kind, the file, and how many lines show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sign {
    /// What kind of weakening.
    pub kind: Kind,
    /// The file inside the project, with `/`.
    pub path: String,
    /// How many lines of the diff show it (removed lines for
    /// [`Kind::FewerAssertions`], 1 for a deleted file).
    pub lines: usize,
}

/// What the diff did to one file.
#[derive(Debug, Default)]
struct FileDiff {
    path: String,
    deleted: bool,
    added: Vec<String>,
    removed: Vec<String>,
}

/// All signs of weakened checks in `patch`, a unified diff as
/// [`crate::git::Repo::patch_of_commit_adding`] returns it. Files under
/// `skip_folder` (the harness's own records) and documents are left out.
pub fn signs(patch: &str, skip_folder: &str) -> Vec<Sign> {
    let files: Vec<FileDiff> = parse(patch)
        .into_iter()
        .filter(|file| !Path::new(&file.path).starts_with(skip_folder) && !is_document(&file.path))
        .collect();
    let mut signs = Vec::new();
    for file in &files {
        for (kind, patterns) in [
            (Kind::TestSkipped, SKIP_PATTERNS),
            (Kind::CheckSilenced, SILENCE_PATTERNS),
            (Kind::Unfinished, UNFINISHED_PATTERNS),
        ] {
            let lines = file
                .added
                .iter()
                .filter(|line| contains_any(line, patterns))
                .count();
            if lines > 0 {
                signs.push(sign(kind, &file.path, lines));
            }
        }
        if file.deleted && permissions::is_test_file(&file.path) && !was_moved(file, &files) {
            signs.push(sign(Kind::TestFileDeleted, &file.path, 1));
        }
    }
    signs.extend(fewer_assertions(&files));
    signs.sort_by(|a, b| (a.kind, &a.path).cmp(&(b.kind, &b.path)));
    signs
}

fn sign(kind: Kind, path: &str, lines: usize) -> Sign {
    Sign {
        kind,
        path: path.to_string(),
        lines,
    }
}

/// When the whole step removed more assertion lines than it added, one sign
/// for each file that lost some. Counting over the whole step means tests
/// that only moved to another file are not reported.
fn fewer_assertions(files: &[FileDiff]) -> Vec<Sign> {
    let added: usize = files.iter().map(|file| assertions(&file.added)).sum();
    let removed: usize = files.iter().map(|file| assertions(&file.removed)).sum();
    if removed <= added {
        return Vec::new();
    }
    files
        .iter()
        .filter_map(|file| {
            let lost = assertions(&file.removed).saturating_sub(assertions(&file.added));
            (lost > 0).then(|| sign(Kind::FewerAssertions, &file.path, lost))
        })
        .collect()
}

/// How many of `lines` look like an assertion in some test framework
/// (`assert!`, `assertEquals`, `self.assertTrue`, `expect(...)`).
fn assertions(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|line| {
            let lower = line.to_lowercase();
            lower.contains("assert") || lower.contains("expect(")
        })
        .count()
}

/// Was the deleted `file` really moved: did the step add a file of the same name?
fn was_moved(file: &FileDiff, files: &[FileDiff]) -> bool {
    let name = file_name(&file.path);
    files
        .iter()
        .any(|other| !other.deleted && other.path != file.path && file_name(&other.path) == name)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_document(path: &str) -> bool {
    let lower = path.to_lowercase();
    DOCUMENT_ENDINGS
        .iter()
        .any(|ending| lower.ends_with(ending))
}

fn contains_any(line: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| line.contains(pattern))
}

/// Splits a unified diff into files. Lines starting with `---` or `+++` are
/// file names only in the header of a file (before its first `@@`); inside a
/// hunk, an added `++i` shows as `+++i` and is content.
fn parse(patch: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut in_header = false;
    for line in patch.lines() {
        if line.starts_with("diff ") {
            files.push(FileDiff::default());
            in_header = true;
            continue;
        }
        // `let ... else` skips lines before the first `diff` line.
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            in_header = false;
        } else if in_header {
            read_header_line(file, line);
        } else if let Some(text) = line.strip_prefix('+') {
            file.added.push(text.to_string());
        } else if let Some(text) = line.strip_prefix('-') {
            file.removed.push(text.to_string());
        }
    }
    files
}

/// Takes the file's path from a `---` / `+++` header line. A deleted file
/// has `+++ /dev/null`, so its path comes from the `---` line.
fn read_header_line(file: &mut FileDiff, line: &str) {
    if let Some(old) = line.strip_prefix("--- ") {
        if old != "/dev/null" {
            file.path = header_path(old);
        }
    } else if let Some(new) = line.strip_prefix("+++ ") {
        if new == "/dev/null" {
            file.deleted = true;
        } else {
            file.path = header_path(new);
        }
    }
}

/// `b/src/a.rs` becomes `src/a.rs`. Git puts a name with unusual characters
/// in quotes (`"b/caf\303\251.rs"`); the quotes are dropped, the escapes stay,
/// which is good enough for a name in a message.
fn header_path(text: &str) -> String {
    let text = text.trim_matches('"');
    text.strip_prefix("a/")
        .or_else(|| text.strip_prefix("b/"))
        .unwrap_or(text)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The diff lines for `removed` and `added`, each with its `-` or `+`.
    fn body(added: &[&str], removed: &[&str]) -> String {
        let removed = removed.iter().map(|line| format!("-{line}\n"));
        let added = added.iter().map(|line| format!("+{line}\n"));
        removed.chain(added).collect()
    }

    /// A small diff of one changed file, as `git show -U0` prints it.
    fn file_patch(path: &str, added: &[&str], removed: &[&str]) -> String {
        let header =
            format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n");
        header + &body(added, removed)
    }

    /// A small diff of one deleted file.
    fn deleted_patch(path: &str, removed: &[&str]) -> String {
        let header = format!(
            "diff --git a/{path} b/{path}\ndeleted file mode 100644\n--- a/{path}\n+++ /dev/null\n@@ -1 +0,0 @@\n"
        );
        header + &body(&[], removed)
    }

    fn kinds(signs: &[Sign]) -> Vec<(Kind, &str, usize)> {
        signs
            .iter()
            .map(|s| (s.kind, s.path.as_str(), s.lines))
            .collect()
    }

    #[test]
    fn a_clean_change_has_no_signs() {
        let patch = file_patch("src/a.rs", &["fn a() -> u8 { 1 }"], &["fn a() -> u8 { 0 }"]);

        assert_eq!(signs(&patch, ".harness"), Vec::<Sign>::new());
    }

    #[test]
    fn skipped_tests_silenced_checks_and_stubs_are_found_in_any_language() {
        let patch = [
            file_patch("tests/parse.rs", &["#[ignore]", "#[test]"], &[]),
            file_patch(
                "web/app.test.ts",
                &["it.skip('works', () => {", "// @ts-ignore"],
                &[],
            ),
            file_patch(
                "svc/handler.py",
                &["    raise NotImplementedError", "x = 1  # noqa"],
                &[],
            ),
            file_patch("cmd/main_test.go", &["\tt.Skip(\"flaky\")"], &[]),
        ]
        .concat();

        let found = signs(&patch, ".harness");

        assert_eq!(
            kinds(&found),
            [
                (Kind::TestSkipped, "cmd/main_test.go", 1),
                (Kind::TestSkipped, "tests/parse.rs", 1),
                (Kind::TestSkipped, "web/app.test.ts", 1),
                (Kind::CheckSilenced, "svc/handler.py", 1),
                (Kind::CheckSilenced, "web/app.test.ts", 1),
                (Kind::Unfinished, "svc/handler.py", 1),
            ]
        );
    }

    #[test]
    fn removed_lines_and_documents_do_not_count_as_skipping() {
        // Taking a skip out makes the checks stronger, not weaker.
        let patch = [
            file_patch("tests/a.rs", &["#[test]"], &["#[ignore]"]),
            file_patch(
                "docs/design.md",
                &["Never use `.skip(` or `#[allow(`."],
                &[],
            ),
        ]
        .concat();

        assert_eq!(signs(&patch, ".harness"), Vec::<Sign>::new());
    }

    #[test]
    fn the_harness_records_are_left_out() {
        let patch = file_patch(".harness/runs/t/notes.json", &["todo!()"], &[]);

        assert_eq!(signs(&patch, ".harness"), Vec::<Sign>::new());
    }

    #[test]
    fn a_deleted_test_file_is_a_sign_but_a_moved_one_is_not() {
        let patch = [
            deleted_patch("tests/old.rs", &["#[test]", "fn old() {}"]),
            deleted_patch("tests/moved.rs", &["#[test]", "fn moved() {}"]),
            file_patch("tests/unit/moved.rs", &["#[test]", "fn moved() {}"], &[]),
            deleted_patch("src/unused.rs", &["fn unused() {}"]),
        ]
        .concat();

        assert_eq!(
            kinds(&signs(&patch, ".harness")),
            [(Kind::TestFileDeleted, "tests/old.rs", 1)]
        );
    }

    #[test]
    fn assertions_taken_out_of_a_test_are_found() {
        let patch = file_patch(
            "tests/sum.rs",
            &["    let total = sum(&[1, 2]);"],
            &[
                "    assert_eq!(sum(&[1, 2]), 3);",
                "    assert!(sum(&[]) == 0);",
            ],
        );

        assert_eq!(
            kinds(&signs(&patch, ".harness")),
            [(Kind::FewerAssertions, "tests/sum.rs", 2)]
        );
    }

    #[test]
    fn assertions_moved_to_another_file_are_not_a_sign() {
        let patch = [
            file_patch("tests/a.rs", &[], &["assert_eq!(a(), 1);"]),
            file_patch("tests/b.rs", &["assert_eq!(a(), 1);"], &[]),
        ]
        .concat();

        assert_eq!(signs(&patch, ".harness"), Vec::<Sign>::new());
    }

    #[test]
    fn plus_signs_inside_a_hunk_are_content_not_a_file_name() {
        // An added line `++i` shows as `+++i`; it must not end the file.
        let patch = file_patch("src/loop.c", &["++i; // eslint-disable-line"], &[]);

        assert_eq!(
            kinds(&signs(&patch, ".harness")),
            [(Kind::CheckSilenced, "src/loop.c", 1)]
        );
    }
}
