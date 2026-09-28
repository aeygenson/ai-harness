//! Which files each role may change. The harness checks this with `git status`
//! after every role, so a rule holds even if an agent ignores its prompt.
//!
//! These are the defaults from docs/design.md (section 3). Later a project will be
//! able to change them in `.harness/harness.toml`.

use crate::handoff::Role;

/// Nobody may change these, whatever the role: the harness's own files, and the
/// files that give agents their instructions and settings. An agent that rewrites
/// them could change how the next agent behaves.
const ALWAYS_FORBIDDEN: &[&str] = &[
    ".harness",
    ".git",
    ".claude",
    ".codex",
    ".gemini",
    // Antigravity CLI reads project skills, rules and MCP servers from here.
    ".agents",
    "CLAUDE.md",
    "AGENTS.md",
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
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.contains(&"..") {
        return false;
    }
    if parts
        .first()
        .is_some_and(|first| ALWAYS_FORBIDDEN.contains(first))
    {
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
