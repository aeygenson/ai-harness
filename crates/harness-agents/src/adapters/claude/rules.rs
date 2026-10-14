//! The tools and permission rules each role gets in Claude Code.

use std::path::Path;

use harness_core::mcp::McpServer;
use harness_core::plugins::Plugin;
use harness_core::task::agent::RoleJob;
use harness_core::task::handoff::Role;
use harness_core::task::permissions::{Commands, RoleSpec, WriteRule};

/// The tools and permission rules one role gets.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RoleRules {
    /// Which built-in tools exist for the agent at all.
    pub(super) tools: Vec<&'static str>,
    /// What it may do without asking. With `dontAsk`, everything else is refused.
    pub(super) allowed: Vec<String>,
    /// Refused even if something above allows it.
    pub(super) denied: Vec<String>,
}

impl RoleRules {
    /// Built from the role's rights ([`Role::spec`]). The harness checks the
    /// result with git anyway; these rules stop a wrong edit before it happens.
    pub(super) fn for_job(job: &RoleJob, servers: &[McpServer], plugins: &[Plugin]) -> Self {
        let read = ["Read(./**)", "Glob", "Grep"].map(String::from);
        let (mut tools, allowed) = if job.role == Role::Human {
            // Lisa is never run as an agent.
            (vec![], vec![])
        } else {
            let inbox = format!("Edit({}/**)", rule_path(&job.project_dir, &job.output_dir));
            let spec = job.role.spec();
            (tools_for(spec), allowed_for(spec, inbox))
        };
        // Every tool of the role's own MCP servers; there are no others.
        let mcp = servers.iter().map(|server| format!("mcp__{}", server.name));
        // Plugins bring skills, which the agent opens with the Skill tool,
        // and possibly MCP servers named `plugin_<plugin>_<server>`.
        let mut from_plugins = Vec::new();
        if !plugins.is_empty() {
            tools.push("Skill");
            from_plugins.push("Skill".to_string());
        }
        for plugin in plugins {
            from_plugins.push(format!("mcp__plugin_{}_*", plugin.name));
        }
        RoleRules {
            tools,
            allowed: read
                .into_iter()
                .chain(allowed)
                .chain(mcp)
                .chain(from_plugins)
                .collect(),
            denied: [
                "Bash(git commit:*)",
                "Bash(git push:*)",
                // The GitHub CLI pushes and opens pull requests with Lisa's
                // login; only the harness does that, after all checks.
                "Bash(gh:*)",
                "Edit(./.git/**)",
                "Edit(./.claude/**)",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

/// The commands a role with [`Commands::Checks`] (Security) may run:
/// reading the change with git, and the vulnerability scanners of common
/// ecosystems; a scanner that is not installed simply fails. They are
/// prefixes, so a scanner's own fix mode would also pass; the git check
/// after the role still catches changed project files.
pub(super) const CHECK_COMMANDS: &[&str] = &[
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

/// The built-in tools a role with these rights gets.
fn tools_for(spec: RoleSpec) -> Vec<&'static str> {
    let mut tools = vec!["Read", "Glob", "Grep"];
    if spec.writes != WriteRule::Nothing {
        tools.push("Edit");
    }
    // Even a role that changes no project file writes its notes.md and
    // handoff.json into its inbox.
    tools.push("Write");
    if spec.commands != Commands::Nothing {
        tools.push("Bash");
    }
    tools
}

/// What a role with these rights may do without asking, besides reading:
/// its edits, its inbox (`inbox` is the rule for it) and its commands.
fn allowed_for(spec: RoleSpec, inbox: String) -> Vec<String> {
    let mut allowed = edit_rules(spec.writes);
    // As before, a role that may change anything gets no separate rule:
    // `Edit(./**)` covers its inbox when the inbox is inside the project,
    // which is where the harness puts it.
    if spec.writes != WriteRule::Anything {
        allowed.push(inbox);
    }
    match spec.commands {
        Commands::Nothing => {}
        Commands::Checks => {
            allowed.extend(CHECK_COMMANDS.iter().map(|c| format!("Bash({c}:*)")));
        }
        Commands::Any => allowed.push("Bash".into()),
    }
    allowed
}

/// Claude's `Edit(...)` rules for `writes`, made from the same rule the
/// harness checks with git, so both always say the same. Each pattern
/// appears twice: `./x` for the project's top folder and `./**/x` for any
/// folder below it.
pub(super) fn edit_rules(writes: WriteRule) -> Vec<String> {
    let (folders, files) = match writes {
        WriteRule::Anything => return vec!["Edit(./**)".into()],
        WriteRule::Nothing => return Vec::new(),
        WriteRule::Only { folders, files } => (folders, files),
    };
    let patterns = folders
        .iter()
        .map(|folder| format!("{folder}/**"))
        .chain(files.iter().map(ToString::to_string));
    patterns
        .flat_map(|pattern| {
            [
                format!("Edit(./{pattern})"),
                format!("Edit(./**/{pattern})"),
            ]
        })
        .collect()
}

/// A path in Claude Code's rule syntax: `./x` inside the project, `//x` absolute.
pub(super) fn rule_path(project_dir: &Path, path: &Path) -> String {
    match path.strip_prefix(project_dir) {
        Ok(relative) => format!("./{}", relative.display()),
        Err(_) => format!("/{}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_rules_follow_the_write_rule() {
        assert_eq!(edit_rules(WriteRule::Anything), ["Edit(./**)"]);
        assert_eq!(edit_rules(WriteRule::Nothing), Vec::<String>::new());
        let docs = WriteRule::Only {
            folders: &["docs"],
            files: &["*.md"],
        };
        assert_eq!(
            edit_rules(docs),
            [
                "Edit(./docs/**)",
                "Edit(./**/docs/**)",
                "Edit(./*.md)",
                "Edit(./**/*.md)"
            ]
        );
    }

    #[test]
    fn a_role_without_commands_gets_no_shell_and_a_reader_no_edit() {
        let reader = RoleSpec {
            writes: WriteRule::Nothing,
            commands: Commands::Nothing,
            network: false,
        };
        assert_eq!(tools_for(reader), ["Read", "Glob", "Grep", "Write"]);
        assert_eq!(allowed_for(reader, "inbox".into()), ["inbox"]);
    }

    #[test]
    fn a_checking_role_may_run_only_the_checks() {
        let checker = RoleSpec {
            writes: WriteRule::Nothing,
            commands: Commands::Checks,
            network: true,
        };
        let allowed = allowed_for(checker, "inbox".into());
        assert_eq!(allowed.len(), 1 + CHECK_COMMANDS.len());
        assert!(allowed.contains(&"Bash(git diff:*)".to_string()));
        assert!(!allowed.contains(&"Bash".to_string()));
    }
}
