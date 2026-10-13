//! What each role may do: which files it may change, which commands it may
//! run and whether it reaches the network ([`Role::spec`]). The harness checks
//! the files with `git status` after every role, so that rule holds even if
//! an agent ignores its prompt.
//!
//! These are the rights from docs/design.md (section 3).

use crate::task::handoff::Role;

/// Nobody may change these, whatever the role: the harness's own files, and the
/// files that give agents their instructions and settings. An agent that rewrites
/// them could change how the next agent behaves.
///
/// They are refused at any depth (`sub/CLAUDE.md`, `pkg/.claude/settings.json`),
/// because agents also read such files from subfolders, and in any letter case,
/// because macOS and Windows treat `Claude.md` and `CLAUDE.md` as the same file.
const ALWAYS_FORBIDDEN: &[&str] = &[
    ".harness",
    ".git",
    ".claude",
    ".codex",
    // Antigravity CLI keeps its settings here (the folder name is Google's).
    ".gemini",
    // Antigravity CLI reads project skills, rules and MCP servers from here.
    ".agents",
    // DeepSeek Harness reads project skills from here.
    ".dsh",
    "CLAUDE.md",
    "AGENTS.md",
    // Antigravity CLI reads it as project instructions.
    "GEMINI.md",
];

/// Names of test files that live next to the code, which the tester may write
/// in any folder. `*` stands for any letters. Many languages keep tests there
/// instead of in a `tests` folder.
pub const TEST_FILE_NAMES: &[&str] = &[
    // Go, Python: parser_test.go
    "*_test.*", // JavaScript, TypeScript: parser.test.ts, parser.spec.ts
    "*.test.*", "*.spec.*", // Ruby: parser_spec.rb
    "*_spec.*", // Python: test_parser.py
    "test_*",   // Java, Kotlin: ParserTest.java; C#, Swift: ParserTests.cs
    "*Test.*", "*Tests.*",
];

/// What one role may write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteRule {
    /// Any file (except the always forbidden ones).
    Anything,
    /// Only files inside a folder with one of these names, at any depth
    /// (`tests` allows `tests/a.rs` and `crates/core/tests/a.rs`), or files
    /// whose own name matches one of `files` (see [`TEST_FILE_NAMES`]).
    Only {
        /// Names of folders the role may write in, for example `tests`.
        folders: &'static [&'static str],
        /// Patterns for file names the role may write anywhere; `*` means any letters.
        files: &'static [&'static str],
    },
    /// Read only.
    Nothing,
}

/// May a role run commands, and which ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commands {
    /// The role needs no commands: it only reads and writes files.
    Nothing,
    /// Only a reviewer's checks, such as `git diff` or a vulnerability scanner.
    Checks,
    /// Any command, such as building and running the tests.
    Any,
}

/// Everything a role is meant to do. Each agent's adapter turns it into what
/// that agent can enforce, and none of them is a full sandbox: Claude Code
/// gets tools and `Edit` rules from `writes` and `commands` (a role with a
/// shell can still write anywhere), Codex gets `network`, Antigravity turns
/// `commands` into a deny list, `dsh` gets none of it. After the role the
/// harness checks the changed project files with git; that check does not
/// see ignored files or files outside the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleSpec {
    /// Which project files the role may change.
    pub writes: WriteRule,
    /// Which commands the role may run.
    pub commands: Commands,
    /// May the role reach the network, for example to download packages.
    pub network: bool,
}

// Rust allows an `impl` block for a type in any module of the crate that
// defines it, so the roles' rights can live here, next to the file checks.
impl Role {
    /// What this role may do. The one place where the roles' rights are written.
    pub fn spec(self) -> RoleSpec {
        match self {
            Role::Architect => RoleSpec {
                writes: WriteRule::Only {
                    folders: &["docs"],
                    files: &[],
                },
                commands: Commands::Nothing,
                network: false,
            },
            Role::Developer => RoleSpec {
                writes: WriteRule::Anything,
                commands: Commands::Any,
                network: true,
            },
            Role::Tester => RoleSpec {
                writes: WriteRule::Only {
                    folders: &["tests"],
                    files: TEST_FILE_NAMES,
                },
                commands: Commands::Any,
                network: true,
            },
            // Security only reads, but runs scanners that may download
            // vulnerability databases.
            Role::Security => RoleSpec {
                writes: WriteRule::Nothing,
                commands: Commands::Checks,
                network: true,
            },
            // Lisa's decisions are written by the harness itself, never by an agent.
            Role::Human => RoleSpec {
                writes: WriteRule::Nothing,
                commands: Commands::Nothing,
                network: false,
            },
        }
    }
}

/// Is `path` one that no role may change: the harness's own files or the
/// files that give agents their instructions and settings (see [`ALWAYS_FORBIDDEN`])?
/// The harness puts such a file back at once when an agent changed it.
pub fn is_protected(path: &str) -> bool {
    let parts = path_parts(path);
    parts.contains(&"..") || parts.iter().any(|part| is_always_forbidden(part))
}

/// Can `role` change the file at `path` (relative to the project, with `/`)?
pub fn may_write(role: Role, path: &str) -> bool {
    if is_protected(path) {
        return false;
    }
    let parts = path_parts(path);
    match role.spec().writes {
        WriteRule::Anything => true,
        WriteRule::Nothing => false,
        WriteRule::Only { folders, files } => {
            let Some((file, path_folders)) = parts.split_last() else {
                return false;
            };
            // For `folders` only folder names count: `src/docs.rs` is not in `docs`.
            path_folders.iter().any(|f| folders.contains(f))
                || files.iter().any(|pattern| name_matches(pattern, file))
        }
    }
}

/// Is `path` a test file: one inside a `tests` folder, or named like a
/// test (see [`TEST_FILE_NAMES`])? These are the files the Tester may write.
pub fn is_test_file(path: &str) -> bool {
    may_write(Role::Tester, path)
}

/// Does the file name `name` match `pattern`, where `*` stands for any
/// letters, also none? `*_test.*` matches `parser_test.go`.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    // `split('*')` cuts the pattern into the fixed pieces between the stars:
    // `*_test.*` becomes `""`, `"_test."`, `""`.
    let mut pieces = pattern.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<&str> = pieces.collect();
    // No star at all: the name must be exactly the pattern.
    let Some((last, middle)) = pieces.split_last() else {
        return rest.is_empty();
    };
    // Each middle piece must come in order; taking the first place it occurs
    // leaves the most room for the pieces after it.
    for piece in middle {
        let Some(at) = rest.find(piece) else {
            return false;
        };
        rest = &rest[at + piece.len()..];
    }
    rest.ends_with(last)
}

/// The folder and file names of `path`. Git always writes `/`; an agent on
/// Windows may write `\\` too.
fn path_parts(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|p| !p.is_empty()).collect()
}

/// Is this one path part (a folder or file name) on the always forbidden list?
fn is_always_forbidden(part: &str) -> bool {
    ALWAYS_FORBIDDEN
        .iter()
        .any(|name| name.eq_ignore_ascii_case(part))
}

/// The files from `changed` that `role` was not allowed to touch.
pub fn forbidden_changes(role: Role, changed: &[String]) -> Vec<String> {
    changed
        .iter()
        .filter(|path| !may_write(role, path))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_files_are_the_ones_the_tester_may_write() {
        assert!(is_test_file("tests/parse.rs"));
        assert!(is_test_file("web/app.test.ts"));
        assert!(is_test_file("pkg/parse_test.go"));
        assert!(!is_test_file("src/parse.rs"));
        assert!(!is_test_file(".harness/tests/x.rs"));
    }

    #[test]
    fn protected_paths_are_the_always_forbidden_ones_at_any_depth() {
        assert!(is_protected(".harness/harness.toml"));
        assert!(is_protected("pkg/Claude.md"));
        assert!(is_protected("sub\\.claude\\settings.json"));
        assert!(is_protected("../outside.txt"));
        assert!(!is_protected("src/main.rs"));
        assert!(!is_protected("docs/claude-notes.md"));
    }

    #[test]
    fn architect_writes_only_docs() {
        assert!(may_write(Role::Architect, "docs/design.md"));
        assert!(may_write(Role::Architect, "crates/core/docs/api.md"));
        assert!(!may_write(Role::Architect, "src/main.rs"));
        assert!(!may_write(Role::Architect, "src/docs.rs"));
        assert!(!may_write(Role::Architect, "README.md"));
    }

    #[test]
    fn developer_writes_anything_but_the_forbidden_files() {
        assert!(may_write(Role::Developer, "src/main.rs"));
        assert!(may_write(Role::Developer, "Cargo.toml"));
        assert!(may_write(Role::Developer, "docs/notes.md"));
        assert!(!may_write(
            Role::Developer,
            ".harness/runs/task-001/state.json"
        ));
        assert!(!may_write(Role::Developer, "CLAUDE.md"));
        assert!(!may_write(Role::Developer, ".claude/settings.json"));
        assert!(!may_write(Role::Developer, ".agents/mcp_config.json"));
    }

    #[test]
    fn tester_writes_only_tests() {
        assert!(may_write(Role::Tester, "tests/parser.rs"));
        assert!(may_write(Role::Tester, "crates/core/tests/parser.rs"));
        assert!(!may_write(Role::Tester, "src/parser.rs"));
        assert!(!may_write(Role::Tester, "tests.rs"));
    }

    #[test]
    fn tester_writes_test_files_next_to_the_code() {
        for path in [
            "parser_test.go",
            "pkg/parser_test.go",
            "src/parser.test.ts",
            "web/parser.spec.js",
            "spec/parser_spec.rb",
            "app/test_parser.py",
            "src/main/java/ParserTest.java",
            "Parser/ParserTests.cs",
        ] {
            assert!(may_write(Role::Tester, path), "{path}");
        }
        for path in [
            "src/latest.go",
            "src/contest.py",
            "src/testing.rs",
            "src/parser.go",
        ] {
            assert!(!may_write(Role::Tester, path), "{path}");
        }
        // The always forbidden files stay forbidden, whatever their name.
        assert!(!may_write(Role::Tester, ".harness/test_state.json"));
    }

    #[test]
    fn a_star_stands_for_any_letters() {
        assert!(name_matches("*_test.*", "a_test.go"));
        assert!(name_matches("*_test.*", "_test."));
        assert!(!name_matches("*_test.*", "a_test"));
        assert!(name_matches("test_*", "test_parser.py"));
        assert!(!name_matches("test_*", "my_test_parser.py"));
        assert!(name_matches("*.spec.*", "a.spec.spec.ts"));
        assert!(name_matches("docs", "docs"));
        assert!(!name_matches("docs", "docs2"));
    }

    #[test]
    fn security_and_human_write_nothing() {
        assert!(!may_write(Role::Security, "docs/security.md"));
        assert!(!may_write(Role::Human, "src/main.rs"));
    }

    #[test]
    fn tricky_paths_are_refused() {
        assert!(!may_write(Role::Developer, "../outside.txt"));
        assert!(!may_write(Role::Tester, "tests/../src/main.rs"));
        assert!(!may_write(Role::Tester, "tests\\..\\src\\main.rs"));
    }

    #[test]
    fn agent_files_are_refused_in_subfolders_too() {
        assert!(!may_write(Role::Developer, "sub/CLAUDE.md"));
        assert!(!may_write(Role::Developer, "packages/web/AGENTS.md"));
        assert!(!may_write(Role::Developer, "pkg/.claude/settings.json"));
        assert!(!may_write(Role::Developer, "vendor/lib/.git/config"));
        assert!(!may_write(Role::Architect, "docs/GEMINI.md"));
        assert!(!may_write(Role::Tester, "tests/.codex/config.toml"));
        // A name that only contains a forbidden one is fine.
        assert!(may_write(Role::Developer, "docs/CLAUDE.md.bak"));
        assert!(may_write(Role::Developer, "src/agents.rs"));
    }

    #[test]
    fn agent_files_are_refused_in_any_letter_case() {
        assert!(!may_write(Role::Developer, "Claude.md"));
        assert!(!may_write(Role::Developer, "agents.md"));
        assert!(!may_write(Role::Developer, ".Claude/settings.json"));
        assert!(!may_write(Role::Developer, "sub/.GIT/config"));
    }

    #[test]
    fn forbidden_changes_lists_only_the_bad_files() {
        let changed = vec![
            "docs/design.md".to_string(),
            "src/main.rs".to_string(),
            ".harness/runs/task-001/state.json".to_string(),
        ];
        assert_eq!(
            forbidden_changes(Role::Architect, &changed),
            ["src/main.rs", ".harness/runs/task-001/state.json"]
        );
    }
}
