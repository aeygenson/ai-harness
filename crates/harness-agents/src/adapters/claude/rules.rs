//! The tools and permission rules each role gets in Claude Code.

use std::path::Path;

use harness_core::task::agent::RoleJob;
use harness_core::task::commands::{CHECK_COMMANDS, DENIED_FOR_ALL, DENIED_FOR_READERS};
use harness_core::task::handoff::Role;
use harness_core::task::permissions::{Commands, RoleSpec, WriteRule};

use crate::role_settings::RoleSettings;

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
    /// `settings` gives the role's MCP servers, plugins and extra commands.
    pub(super) fn for_job(job: &RoleJob, settings: &RoleSettings) -> Self {
        let servers = settings.servers(job.role);
        let plugins = settings.plugins(job.role);
        let read = ["Read(./**)", "Glob", "Grep"].map(String::from);
        let (mut tools, allowed) = if job.role == Role::Human {
            // The user is never run as an agent.
            (vec![], vec![])
        } else {
            let inbox = format!("Edit({}/**)", rule_path(&job.project_dir, &job.output_dir));
            let spec = job.role.spec();
            let extra = settings.extra_commands(job.role);
            (tools_for(spec), allowed_for(spec, inbox, extra))
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
            denied: denied_for(job.role),
        }
    }
}

/// What the role may never do, even if a rule above allows it: deny beats
/// allow in Claude Code. A role without every command (Security) is also
/// denied the commands a reading role may not run, so a broad extra command
/// still cannot reach them.
fn denied_for(role: Role) -> Vec<String> {
    let mut commands: Vec<&str> = DENIED_FOR_ALL.to_vec();
    if role.spec().commands != Commands::Any {
        commands.extend(DENIED_FOR_READERS);
    }
    let mut denied: Vec<String> = commands.iter().map(|c| format!("Bash({c}:*)")).collect();
    denied.extend(["Edit(./.git/**)", "Edit(./.claude/**)"].map(String::from));
    denied
}

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
/// `extra` are the user's own checks; only a checking role gets them
/// (`Config::parse` refuses them for any other role).
fn allowed_for(spec: RoleSpec, inbox: String, extra: &[String]) -> Vec<String> {
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
            let checks = CHECK_COMMANDS.iter().copied();
            let extra = extra.iter().map(String::as_str);
            allowed.extend(checks.chain(extra).map(|c| format!("Bash({c}:*)")));
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
        assert_eq!(allowed_for(reader, "inbox".into(), &[]), ["inbox"]);
    }

    #[test]
    fn a_checking_role_may_run_only_the_checks() {
        let checker = RoleSpec {
            writes: WriteRule::Nothing,
            commands: Commands::Checks,
            network: true,
        };
        let allowed = allowed_for(checker, "inbox".into(), &[]);
        assert_eq!(allowed.len(), 1 + CHECK_COMMANDS.len());
        assert!(allowed.contains(&"Bash(git diff:*)".to_string()));
        assert!(!allowed.contains(&"Bash".to_string()));
    }

    #[test]
    fn a_checking_role_is_denied_what_readers_may_not_run_a_developer_only_the_rest() {
        let security = denied_for(Role::Security);
        assert!(security.contains(&"Bash(git commit:*)".to_string()));
        assert!(security.contains(&"Bash(git config:*)".to_string()));
        assert!(security.contains(&"Bash(rm:*)".to_string()));
        let developer = denied_for(Role::Developer);
        assert!(developer.contains(&"Bash(gh:*)".to_string()));
        assert!(!developer.contains(&"Bash(rm:*)".to_string()));
    }

    #[test]
    fn a_checking_role_also_gets_the_users_extra_commands() {
        let checker = Role::Security.spec();
        let extra = ["trivy fs".to_string()];
        let allowed = allowed_for(checker, "inbox".into(), &extra);
        assert!(allowed.contains(&"Bash(trivy fs:*)".to_string()));
        assert_eq!(allowed.len(), 2 + CHECK_COMMANDS.len());
    }

    #[test]
    fn a_role_with_any_command_gets_the_whole_shell_not_the_extra_list() {
        let extra = ["trivy fs".to_string()];
        let allowed = allowed_for(Role::Developer.spec(), "inbox".into(), &extra);
        assert!(allowed.contains(&"Bash".to_string()));
        assert!(!allowed.contains(&"Bash(trivy fs:*)".to_string()));
    }
}
