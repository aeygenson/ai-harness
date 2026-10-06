//! The tools and permission rules each role gets in Claude Code.

use std::path::Path;

use harness_core::mcp::McpServer;
use harness_core::plugins::Plugin;
use harness_core::task::agent::RoleJob;
use harness_core::task::handoff::Role;
use harness_core::task::permissions::{self, WriteRule};

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
    /// Mirrors `harness_core::task::permissions`. The harness checks the result with
    /// git anyway; these rules stop a wrong edit before it happens.
    pub(super) fn for_job(job: &RoleJob, servers: &[McpServer], plugins: &[Plugin]) -> Self {
        let inbox = format!("Edit({}/**)", rule_path(&job.project_dir, &job.output_dir));
        let read = ["Read(./**)", "Glob", "Grep"].map(String::from);
        let (mut tools, allowed): (Vec<&str>, Vec<String>) = match job.role {
            Role::Architect => (
                vec!["Read", "Glob", "Grep", "Edit", "Write"],
                edit_rules(Role::Architect)
                    .into_iter()
                    .chain([inbox])
                    .collect(),
            ),
            Role::Developer => (
                vec!["Read", "Glob", "Grep", "Edit", "Write", "Bash"],
                vec!["Edit(./**)".into(), "Bash".into()],
            ),
            Role::Tester => (
                vec!["Read", "Glob", "Grep", "Edit", "Write", "Bash"],
                edit_rules(Role::Tester)
                    .into_iter()
                    .chain([inbox, "Bash".into()])
                    .collect(),
            ),
            Role::Security => (
                vec!["Read", "Glob", "Grep", "Write", "Bash"],
                [inbox]
                    .into_iter()
                    .chain(SECURITY_COMMANDS.iter().map(|c| format!("Bash({c}:*)")))
                    .collect(),
            ),
            // Lisa is never run as an agent.
            Role::Human => (vec![], vec![]),
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
                "Edit(./.git/**)",
                "Edit(./.claude/**)",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

/// The commands Security may run: reading the change with git, and the
/// vulnerability scanners of common ecosystems. They only read, so Security
/// stays a role that changes nothing; a scanner that is not installed simply fails.
pub(super) const SECURITY_COMMANDS: &[&str] = &[
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

/// Claude's `Edit(...)` rules for what `role` may write, made from
/// `harness_core::task::permissions`, so both always say the same. Each pattern
/// appears twice: `./x` for the project's top folder and `./**/x` for any
/// folder below it.
pub(super) fn edit_rules(role: Role) -> Vec<String> {
    let WriteRule::Only { folders, files } = permissions::rule_for(role) else {
        return Vec::new();
    };
    let patterns = folders
        .iter()
        .map(|folder| format!("{folder}/**"))
        .chain(files.iter().map(|file| file.to_string()));
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
