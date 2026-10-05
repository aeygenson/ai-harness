//! Which files each role may change. The harness checks this with `git status`
//! after every role, so a rule holds even if an agent ignores its prompt.
//!
//! These are the defaults from docs/design.md (section 3). Later a project will be
//! able to change them in `.harness/harness.toml`.

use crate::handoff::Role;

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

/// What one role may write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteRule {
    /// Any file (except the always forbidden ones).
    Anything,
    /// Only files inside a folder with one of these names, at any depth:
    /// `tests` allows `tests/a.rs` and `crates/core/tests/a.rs`.
    FoldersNamed(&'static [&'static str]),
    /// Read only.
    Nothing,
}

pub fn rule_for(role: Role) -> WriteRule {
    match role {
        Role::Architect => WriteRule::FoldersNamed(&["docs"]),
        Role::Developer => WriteRule::Anything,
        Role::Tester => WriteRule::FoldersNamed(&["tests"]),
        Role::Security => WriteRule::Nothing,
        // Lisa's decisions are written by the harness itself, never by an agent.
        Role::Human => WriteRule::Nothing,
    }
}

/// Can `role` change the file at `path` (relative to the project, with `/`)?
pub fn may_write(role: Role, path: &str) -> bool {
    // Git always writes `/`; an agent on Windows may write `\` too.
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if parts.contains(&"..") {
        return false;
    }
    if parts.iter().any(|part| is_always_forbidden(part)) {
        return false;
    }
    match rule_for(role) {
        WriteRule::Anything => true,
        WriteRule::Nothing => false,
        // Only folder names count, not the file name: `src/docs.rs` is not in `docs`.
        WriteRule::FoldersNamed(names) => parts
            .split_last()
            .is_some_and(|(_file, folders)| folders.iter().any(|f| names.contains(f))),
    }
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
