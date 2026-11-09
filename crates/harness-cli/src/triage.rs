//! `harness triage`: starts the user's own agent (Claude Code, Codex or
//! Cursor) in the terminal with a ready playbook (`triage.md`), so it finds
//! out why a task went wrong and says what to do next.
//!
//! This agent is not a role: it runs with the user's own login and settings,
//! like typing `claude` in the terminal, in the project folder. The playbook
//! asks it to only read and report until the user agrees to a change.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use harness_core::config::AgentKind;

use crate::{open_repo, open_task};

/// The playbook the agent gets as its first message; `{...}` are filled in.
const PLAYBOOK: &str = include_str!("triage.md");

/// The agents that can triage, in the order tried when none is named. They
/// all start an interactive session when given a first message.
const TRIAGE_AGENTS: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor];

/// Finds a program by name; tests give a fake one.
type Finder = fn(&str) -> Option<PathBuf>;

/// Runs `harness triage`: checks the task, picks the agent, starts it in this
/// terminal and waits until the user closes it.
pub(crate) fn triage(
    project: &Path,
    task_id: Option<&str>,
    agent: Option<AgentKind>,
) -> Result<()> {
    let repo = open_repo(project)?;
    if let Some(task_id) = task_id {
        // A wrong id stops here, before an agent starts and searches in vain.
        open_task(&repo, task_id)?;
    }
    let (agent, program) = pick_agent(agent, harness_platform::program::find)?;
    // The full path, not `.`: the agent reads it as a fact about the project.
    let root = std::path::absolute(repo.root()).context("cannot find the project folder")?;
    match task_id {
        Some(id) => println!("Starting {agent} to look into {id}..."),
        None => println!("Starting {agent}; it will ask which task to look into..."),
    }
    let text = playbook(env!("CARGO_PKG_VERSION"), &root, task_id);
    let status = Command::new(&program)
        .arg(fit_for(&program, &text))
        .current_dir(&root)
        .status()
        .with_context(|| format!("cannot start {}", program.display()))?;
    if !status.success() {
        bail!("{agent} ended with an error ({status})");
    }
    Ok(())
}

/// The agent to use and its program: the one asked for, or else the first of
/// [`TRIAGE_AGENTS`] that is installed.
fn pick_agent(wanted: Option<AgentKind>, find: Finder) -> Result<(AgentKind, PathBuf)> {
    let Some(agent) = wanted else {
        for agent in TRIAGE_AGENTS {
            if let Some(program) = find_program(agent, find) {
                return Ok((agent, program));
            }
        }
        bail!("no agent for triage is installed: install Claude Code, Codex or Cursor CLI");
    };
    if !TRIAGE_AGENTS.contains(&agent) {
        bail!("{agent} cannot triage; use claude, codex or cursor");
    }
    match find_program(agent, find) {
        Some(program) => Ok((agent, program)),
        None => bail!("{agent} is not installed; see `harness agents`"),
    }
}

/// The installed program of `agent`, if any.
fn find_program(agent: AgentKind, find: Finder) -> Option<PathBuf> {
    match agent {
        AgentKind::Claude => find("claude"),
        AgentKind::Codex => find("codex"),
        AgentKind::Cursor => harness_agents::adapters::cursor::PROGRAMS
            .iter()
            .find_map(|name| find(name)),
        AgentKind::Antigravity | AgentKind::Dsh | AgentKind::DshNvidia => None,
    }
}

/// The playbook with the version, the project folder and the task filled in.
fn playbook(version: &str, project: &Path, task_id: Option<&str>) -> String {
    PLAYBOOK
        .replace("{version}", version)
        .replace("{project}", &project.display().to_string())
        .replace("{task}", &task_text(task_id))
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

/// The task as the playbook names it.
fn task_text(task_id: Option<&str>) -> String {
    match task_id {
        Some(id) => id.to_string(),
        None => "not named (ask the user, or look at the newest one in .harness/runs/)".to_string(),
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

        assert!(
            missing.to_string().contains("claude is not installed"),
            "{missing}"
        );
        assert!(unable.to_string().contains("dsh cannot triage"), "{unable}");
        assert!(none.to_string().contains("no agent for triage"), "{none}");
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
    fn without_a_task_the_agent_is_told_to_ask() {
        let text = playbook("1.2.3", Path::new("/work/app"), None);

        assert!(text.contains("task: not named (ask the user"));
    }
}
