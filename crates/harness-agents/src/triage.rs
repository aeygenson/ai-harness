//! Triage: the user's own agent (Claude Code, Codex or Cursor) finds out why
//! a task went wrong, with a ready playbook (`triage.md`). `harness triage`
//! and the «Triage» button of the TUI's Tasks tab both start it here.
//!
//! This agent is not a role: it runs with the user's own login and settings,
//! like typing `claude` in the terminal, in the project folder. The playbook
//! asks it to only read and report until the user agrees to a change.

use std::path::{Path, PathBuf};
use std::process::Command;

use harness_core::config::AgentKind;

/// The playbook the agent gets as its first message; `{...}` are filled in.
const PLAYBOOK: &str = include_str!("triage.md");

/// The agents that can triage, in the order tried when none is named. They
/// all start an interactive session when given a first message.
pub const AGENTS: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor];

/// Finds a program by name: `harness_platform::program::find`, or a fake in tests.
pub type Finder = fn(&str) -> Option<PathBuf>;

/// The agent to use and its program: the one asked for, or else the first of
/// [`AGENTS`] that is installed. `Err` says what to install or choose.
pub fn pick_agent(wanted: Option<AgentKind>, find: Finder) -> Result<(AgentKind, PathBuf), String> {
    let Some(agent) = wanted else {
        for agent in AGENTS {
            if let Some(program) = find_program(agent, find) {
                return Ok((agent, program));
            }
        }
        return Err(
            "no agent for triage is installed: install Claude Code, Codex or Cursor CLI".into(),
        );
    };
    if !AGENTS.contains(&agent) {
        return Err(format!(
            "{agent} cannot triage; use claude, codex or cursor"
        ));
    }
    find_program(agent, find).map_or_else(
        || Err(format!("{agent} is not installed; see `harness agents`")),
        |program| Ok((agent, program)),
    )
}

/// The command that starts `program` in `project` with the playbook for
/// `task_id` (or without a task: the agent asks). It reads the keyboard and
/// writes to the terminal itself, so the caller gives it the terminal.
pub fn command(program: &Path, project: &Path, task_id: Option<&str>) -> Command {
    // The full path, not `.`: the agent reads it as a fact about the project.
    let project = std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf());
    let text = playbook(env!("CARGO_PKG_VERSION"), &project, task_id);
    let mut command = Command::new(program);
    command.arg(fit_for(program, &text)).current_dir(&project);
    command
}

/// The installed program of `agent`, if any.
fn find_program(agent: AgentKind, find: Finder) -> Option<PathBuf> {
    match agent {
        AgentKind::Claude => find("claude"),
        AgentKind::Codex => find("codex"),
        AgentKind::Cursor => crate::adapters::cursor::PROGRAMS
            .iter()
            .find_map(|name| find(name)),
        AgentKind::Antigravity | AgentKind::Dsh | AgentKind::DshNvidia => None,
    }
}

/// The playbook with the version, the project folder and the task filled in.
fn playbook(version: &str, project: &Path, task_id: Option<&str>) -> String {
    let task =
        task_id.unwrap_or("not named (ask the user, or look at the newest one in .harness/runs/)");
    PLAYBOOK
        .replace("{version}", version)
        .replace("{project}", &project.display().to_string())
        .replace("{task}", task)
}

/// The playbook as `program` can take it. A Windows `.cmd` or `.bat` program
/// (Codex and Cursor installed with npm) runs through `cmd.exe`, and Rust
/// refuses to pass it an argument with line breaks: there the text becomes
/// one line. Other programs get it as it is.
fn fit_for(program: &Path, text: &str) -> String {
    let batch = program
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
    if batch {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only_codex(name: &str) -> Option<PathBuf> {
        (name == "codex").then(|| PathBuf::from("/bin/codex"))
    }

    fn nothing(_: &str) -> Option<PathBuf> {
        None
    }

    #[test]
    fn without_a_name_the_first_installed_agent_triages() {
        let (agent, program) = pick_agent(None, only_codex).unwrap();

        assert_eq!(agent, AgentKind::Codex);
        assert_eq!(program, PathBuf::from("/bin/codex"));
    }

    #[test]
    fn a_named_agent_must_be_installed_and_able_to_triage() {
        let missing = pick_agent(Some(AgentKind::Claude), only_codex).unwrap_err();
        let unable = pick_agent(Some(AgentKind::Dsh), only_codex).unwrap_err();
        let none = pick_agent(None, nothing).unwrap_err();

        assert!(missing.contains("claude is not installed"), "{missing}");
        assert!(unable.contains("dsh cannot triage"), "{unable}");
        assert!(none.contains("no agent for triage"), "{none}");
    }

    #[test]
    fn cursor_is_found_by_either_of_its_names() {
        let found = pick_agent(Some(AgentKind::Cursor), |name| {
            (name == "agent").then(|| PathBuf::from("/bin/agent"))
        });

        assert_eq!(found.unwrap().1, PathBuf::from("/bin/agent"));
    }

    #[test]
    fn the_playbook_names_the_version_project_and_task() {
        let text = playbook("1.2.3", Path::new("/work/app"), Some("task-007"));

        assert!(text.contains("harness version: 1.2.3"));
        assert!(text.contains("(tag v1.2.3)"));
        assert!(text.contains("project folder (the current folder): /work/app"));
        assert!(text.contains("task: task-007"));
        assert!(!text.contains('{'), "a placeholder was left: {text}");
    }

    #[test]
    fn without_a_task_the_agent_is_told_to_ask() {
        let text = playbook("1.2.3", Path::new("/work/app"), None);

        assert!(text.contains("task: not named (ask the user"));
    }

    #[test]
    fn a_windows_batch_program_gets_the_playbook_on_one_line() {
        let text = "Steps:\n1. Ask.\n2. Read.";

        assert_eq!(
            fit_for(Path::new("codex.CMD"), text),
            "Steps: 1. Ask. 2. Read."
        );
        assert_eq!(fit_for(Path::new("claude.exe"), text), text);
        assert_eq!(fit_for(Path::new("/usr/bin/claude"), text), text);
    }

    #[test]
    fn the_command_starts_in_the_project_with_the_playbook() {
        let command = command(
            Path::new("/bin/claude"),
            Path::new("/work/app"),
            Some("task-007"),
        );

        assert_eq!(command.get_program(), "/bin/claude");
        let arg = command
            .get_args()
            .next()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(arg.contains("task: task-007"), "{arg}");
        assert!(command.get_current_dir().unwrap().ends_with("app"));
    }
}
