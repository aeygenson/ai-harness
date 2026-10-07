//! `harness doctor`: is everything the harness needs on this computer? The
//! programs (Git, Node.js, ...) and the agents, each with what to do when
//! something is missing. The TUI shows the same on the Agents tab. Inside a
//! project it also checks the project's roles.

use std::path::Path;

use anyhow::{bail, Result};
use harness_agents::install::catalog::{self, Status};
use harness_agents::install::credentials;
use harness_agents::install::tools::{self, Health, Tool, ToolStatus};
use harness_core::config::{independence, Config};
use harness_core::git::{Repo, HARNESS_DIR};

/// Width of the name column, enough for «`DeepSeek` Harness».
const NAME_WIDTH: usize = 18;

/// Checks the computer, prints what was found and fails when something
/// required is missing (so a script can tell).
pub(crate) fn doctor(project: &Path) -> Result<()> {
    println!("Checking this computer…");
    let dir = credentials::default_dir();
    let (tools, agents) = std::thread::scope(|scope| {
        let tools = scope.spawn(tools::check);
        let agents = catalog::check_all(dir.as_deref());
        (tools.join().unwrap_or_default(), agents)
    });
    let (text, problems) = report(&tools, &agents);
    print!("{text}");
    if let Some(config) = project_config(project) {
        print!("{}", roles_report(&config));
    }
    if problems > 0 {
        bail!("{problems} problem(s) to fix, see above");
    }
    Ok(())
}

/// The settings of the project `dir` is in; `None` outside a project.
fn project_config(dir: &Path) -> Option<Config> {
    let repo = Repo::open(dir).ok()?;
    Config::load(&repo.root().join(HARNESS_DIR)).ok()
}

/// What the project's roles look like: only warnings, never a problem.
fn roles_report(config: &Config) -> String {
    let line = if independence::security_same_as_developer(&config.roles) {
        "! Security uses the same agent and model as the Developer, so its check is less \
         independent. Better give it another agent (harness tui, Roles tab).\n"
    } else {
        "✓ Security uses another agent or model than the Developer.\n"
    };
    format!("\nThis project's roles:\n{line}")
}

/// The report for `tools` and `agents`, and how many problems it found:
/// a missing required tool, or no agent ready for a role.
fn report(tools: &[ToolStatus], agents: &[Status]) -> (String, usize) {
    let mut text = String::from("\nPrograms:\n");
    let mut problems = 0;
    for status in tools {
        let health = status.health();
        if health == Health::Missing {
            problems += 1;
        }
        text.push_str(&tool_line(status, health));
    }
    if tools.iter().any(|s| s.health() != Health::Good) {
        text.push_str(
            "  Install what is missing with the harness's installer (safe to run again):\n",
        );
        text.push_str("    ");
        text.push_str(harness_platform::program::installer_command());
        text.push('\n');
    }

    text.push_str("\nAgents:\n");
    for status in agents.iter().filter(|s| s.entry.runs()) {
        text.push_str(&agent_line(status));
    }
    if agents.iter().any(Status::ready) {
        text.push_str("  Roles can use the agents marked ✓.\n");
    } else {
        problems += 1;
        text.push_str("✗ No agent is ready: install one and sign in on the Agents tab (harness tui, key 8).\n");
    }

    text.push('\n');
    if problems == 0 {
        text.push_str("Everything the harness needs is here.\n");
    }
    (text, problems)
}

/// One line for a tool: its mark, name and version, or what is wrong.
fn tool_line(status: &ToolStatus, health: Health) -> String {
    let mark = match health {
        Health::Good => "✓",
        Health::Warning => "!",
        Health::Missing => "✗",
    };
    let name = status.tool.name();
    let about = match (&status.path, &status.version, status.tool.min_version()) {
        (None, _, _) => format!("not found: {}", purpose(status.tool)),
        (Some(_), Some(version), Some(min)) if status.old() => {
            format!("{version}, older than {min}: {}", purpose(status.tool))
        }
        (Some(_), Some(version), _) => version.clone(),
        (Some(path), None, _) => path.display().to_string(),
    };
    format!("{mark} {name:<NAME_WIDTH$} {about}\n")
}

/// What a tool is for, said when it is missing or too old.
fn purpose(tool: Tool) -> &'static str {
    match tool {
        Tool::Git => "every role's work is committed with git",
        Tool::Node => "Codex CLI, DeepSeek Harness (22.19 or newer) and MCP servers need it",
        Tool::Npx => "most MCP servers are started with npx",
        Tool::Curl => "the MCP registry, model lists and web MCP servers use it",
        Tool::Bubblewrap => "without it the commands Claude Code runs can see its login",
        Tool::Zed => "files open in another editor instead",
    }
}

/// One line for an agent the harness runs: ready, needs something, or not installed.
fn agent_line(status: &Status) -> String {
    let name = status.entry.name;
    let version = status.version.as_deref().unwrap_or("?");
    if !status.installed() {
        return format!("○ {name:<NAME_WIDTH$} not installed\n");
    }
    if let (true, Some(min)) = (status.old(), status.entry.min_version) {
        return format!(
            "! {name:<NAME_WIDTH$} {version}, older than {min}: update it on the Agents tab\n"
        );
    }
    if status.ready() {
        format!("✓ {name:<NAME_WIDTH$} {version}, signed in\n")
    } else {
        format!(
            "! {name:<NAME_WIDTH$} {version}, not signed in: press «Sign in» on the Agents tab\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn tool(tool: Tool, version: Option<&str>) -> ToolStatus {
        ToolStatus {
            tool,
            path: version.map(|_| PathBuf::from("/bin/x")),
            version: version.map(str::to_string),
        }
    }

    /// Claude Code installed and signed in, Codex too old, the rest missing.
    fn agents(signed_in: bool) -> Vec<Status> {
        let creds = tempfile::tempdir().unwrap();
        if signed_in {
            credentials::save_token(creds.path(), "claude", &credentials::Secret::new("t"))
                .unwrap();
        }
        let find = |name: &str| match name {
            "claude" | "codex" => Some(PathBuf::from(format!("/bin/{name}"))),
            _ => None,
        };
        let version = |path: &Path| {
            Ok(if path.ends_with("codex") {
                "codex-cli 0.150.0".to_string()
            } else {
                "2.1.300 (Claude Code)".to_string()
            })
        };
        catalog::check_with(catalog::CATALOG, &find, &version, Some(creds.path()))
    }

    #[test]
    fn a_ready_computer_has_no_problems() {
        let tools = [
            tool(Tool::Git, Some("2.43.0")),
            tool(Tool::Node, Some("22.20.0")),
        ];

        let (text, problems) = report(&tools, &agents(true));

        assert_eq!(problems, 0, "{text}");
        assert!(text.contains("✓ Git                2.43.0"), "{text}");
        assert!(
            text.contains("✓ Claude Code        2.1.300, signed in"),
            "{text}"
        );
        assert!(
            text.contains("! Codex CLI          0.150.0, older than 0.158.0"),
            "{text}"
        );
        assert!(
            text.contains("○ Antigravity CLI    not installed"),
            "{text}"
        );
        assert!(!text.contains("installer"), "{text}");
        assert!(
            text.contains("Everything the harness needs is here."),
            "{text}"
        );
    }

    #[test]
    fn missing_programs_and_no_ready_agent_are_problems_with_a_fix() {
        let tools = [
            tool(Tool::Git, None),
            tool(Tool::Node, Some("20.11.1")),
            tool(Tool::Zed, None),
        ];

        let (text, problems) = report(&tools, &agents(false));

        // Git missing and no agent ready; old Node and missing Zed are only warnings.
        assert_eq!(problems, 2, "{text}");
        assert!(text.contains("✗ Git                not found"), "{text}");
        assert!(
            text.contains("! Node.js            20.11.1, older than 22.19"),
            "{text}"
        );
        assert!(text.contains("! Zed                not found"), "{text}");
        assert!(
            text.contains(harness_platform::program::installer_command()),
            "{text}"
        );
        assert!(text.contains("not signed in: press «Sign in»"), "{text}");
        assert!(text.contains("✗ No agent is ready"), "{text}");
    }

    #[test]
    fn security_on_the_developers_agent_and_model_is_a_warning() {
        let text = "[roles.developer]\nagent = \"claude\"\n\
                    [roles.security]\nagent = \"claude\"\n";
        let mut config = Config::parse(text).unwrap();

        let same = roles_report(&config);
        config
            .roles
            .get_mut(&harness_core::task::handoff::Role::Security)
            .unwrap()
            .agent = harness_core::config::AgentKind::Codex;
        let other = roles_report(&config);

        assert!(
            same.contains("! Security uses the same agent and model"),
            "{same}"
        );
        assert!(other.contains("✓ Security uses another agent"), "{other}");
    }
}
