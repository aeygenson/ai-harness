//! `harness task new`, `run`, `approve`, `reject` and `status`: moving a task
//! through the roles.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use harness_agents::build::build_team;
use harness_core::config::Config;
use harness_core::git::HARNESS_DIR;
use harness_core::skills::Skills;
use harness_core::task::handoff::{NextStep, Role, Verdict};
use harness_core::task::orchestrator::{self, Project, StopReason};
use harness_core::task::{Stage, WaitReason};
use harness_platform::stop::StopSignals;

use crate::{open_repo, open_task};

/// `harness task new`: creates the task folder and commits it. Refuses a
/// description an unfinished task already has, unless `allow_duplicate`.
pub(crate) fn new_task(
    project: &Path,
    task_id: &str,
    description: &str,
    allow_duplicate: bool,
) -> Result<()> {
    let repo = open_repo(project)?;
    let config = Config::load(&repo.root().join(HARNESS_DIR))?;
    let create = if allow_duplicate {
        orchestrator::create_task_anyway
    } else {
        orchestrator::create_task
    };
    let (store, _) = create(&repo, task_id, description, config.max_rounds)?;
    println!(
        "Created {}. Next: harness run {task_id}",
        store.dir().display()
    );
    Ok(())
}

/// Waits for `work` (which runs agents) until it ends or the harness is asked
/// to stop: Ctrl+C, the terminal window closed, `kill` (see `harness_platform::stop`).
///
/// Each agent runs in its own process group, so these signals reach only the
/// harness. Here the harness stops waiting, which drops `work`, and dropping
/// it stops the running agent with everything it started (see
/// `harness_agents::process::run`).
pub(crate) async fn until_stopped(
    work: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    let mut signals = StopSignals::listen().context("cannot listen for Ctrl+C")?;
    // `select!` waits for whichever finishes first and drops the other one.
    tokio::select! {
        result = work => result,
        () = signals.recv() => {
            bail!("stopped (Ctrl+C or the terminal closed); the agent and everything it started were stopped")
        }
    }
}

/// `harness run`: runs the roles one after another until the task is done
/// or waits for Lisa, then says what to do next.
pub(crate) async fn run(project: &Path, task_id: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let config = Config::load(&harness_dir)?;
    let skills = Skills::load(&harness_dir, &config)?;
    let agent = build_team(&config, repo.root())?;
    let (store, mut state) = open_task(&repo, task_id)?;
    println!("Running {task_id} (round {})...", state.round);
    let project = Project {
        config: &config,
        skills: &skills,
        agent_versions: agent.versions(),
    };
    let stop = orchestrator::run_project(&repo, &store, &mut state, &agent, project).await?;
    println!("{}", explain(&stop, task_id));
    Ok(())
}

/// The message telling Lisa why the run stopped and which command comes next.
fn explain(stop: &StopReason, task_id: &str) -> String {
    match stop {
        StopReason::Done => "Done: security approved the work.".into(),
        StopReason::WaitingForHuman(WaitReason::ApproveDesign) => format!(
            "The architect's design is ready. Read it, then:\n  \
             harness approve {task_id}   or   harness reject {task_id} --to architect --notes \"...\""
        ),
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(role)) => format!(
            "The {role:?} needs your help; see its notes.md. Answer with:\n  \
             harness approve {task_id} --notes \"...\""
        ),
        StopReason::WaitingForHuman(WaitReason::RoundLimitReached) => format!(
            "The round limit is reached. Decide who continues:\n  \
             harness approve {task_id} --to <role|done> --notes \"...\""
        ),
        StopReason::UsageLimitReached(role) => format!(
            "The subscription limit stopped the {role:?}. Nothing was saved; run again later."
        ),
        StopReason::RoleFailed { role, problem } => {
            format!("The {role:?} failed twice. Last problem:\n{problem}")
        }
        StopReason::StepLimitReached => "Stopped after too many steps in one run.".into(),
        StopReason::DirtyWorkingTree(files) => format!(
            "The project has uncommitted changes. Commit or remove them first:\n  {}",
            files.join("\n  ")
        ),
        StopReason::ForbiddenChanges { role, files } => format!(
            "The {role:?} changed files it may not touch (left uncommitted for you to check):\n  {}",
            files.join("\n  ")
        ),
        StopReason::TooLarge { role, files } => format!(
            "The {role:?} made files too big to commit (left uncommitted). Usually this is build \
             output: add it to .gitignore or delete it, commit or remove the rest, then run again:\n  {}",
            files
                .iter()
                .map(|(path, bytes)| format!("{path} ({} MB)", bytes.div_ceil(1024 * 1024)))
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
        StopReason::AgentCommitted(role) => format!(
            "The {role:?} made a git commit itself. Check `git log` before running again."
        ),
        StopReason::ProtectedFilesChanged { role, files, log } => format!(
            "The {role:?} changed files no role may change (agent instructions and settings, \
             the harness's own files). They are put back; what it wrote is in {}. Its other \
             changes are left uncommitted for you to check:\n  {}",
            log.display(),
            files.join("\n  ")
        ),
        StopReason::HiddenChanges {
            role,
            put_back,
            reported,
        } => hidden_changes_text(*role, put_back, reported),
        StopReason::GitConfigChanged(role) => format!(
            "The {role:?} changed the project's git settings (.git/config). The old settings \
             are back; its other changes are left uncommitted for you to check."
        ),
    }
}

/// What `StopReason::HiddenChanges` says: git's files that were put back,
/// and files outside the project Lisa should look at herself.
fn hidden_changes_text(role: Role, put_back: &[PathBuf], reported: &[PathBuf]) -> String {
    let list = |files: &[PathBuf]| -> String {
        let lines: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
        format!("\n  {}", lines.join("\n  "))
    };
    let mut text = format!("The {role:?} changed files that git does not show.");
    if !put_back.is_empty() {
        text.push_str("\nGit's own files (hooks, info/), now put back as they were:");
        text.push_str(&list(put_back));
    }
    if !reported.is_empty() {
        text.push_str(
            "\nFiles outside the project, NOT put back; check them yourself (if you \
             changed them during the run, all is well):",
        );
        text.push_str(&list(reported));
    }
    text.push_str("\nThe role's project changes are left uncommitted for you to check.");
    text
}

/// `harness approve` / `harness reject`: writes Lisa's decision as a handoff
/// from the role `human`. Without `--to`, an approved design goes to the
/// developer and a role that asked for help gets the task back.
pub(crate) fn decide(
    project: &Path,
    task_id: &str,
    verdict: Verdict,
    to: Option<NextStep>,
    notes: &str,
) -> Result<()> {
    let repo = open_repo(project)?;
    let (store, mut state) = open_task(&repo, task_id)?;
    let next = match to {
        Some(next) => next,
        None => match state.stage {
            Stage::WaitingForHuman(WaitReason::ApproveDesign) => NextStep::To(Role::Developer),
            Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(role)) => NextStep::To(role),
            _ => bail!("say who continues with --to <role|done>"),
        },
    };
    orchestrator::record_human_decision(&repo, &store, &mut state, verdict, next, notes)?;
    println!("Saved. Next: harness run {task_id}");
    Ok(())
}

/// `harness status`: the task's round and stage, and one line per handoff.
pub(crate) fn status(project: &Path, task_id: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let (store, state) = open_task(&repo, task_id)?;
    println!(
        "{task_id}: round {} of {}, {:?}",
        state.round, state.max_rounds, state.stage
    );
    for handoff in store.history()? {
        println!(
            "  round {} {:?} {:?} -> {:?}: {}",
            handoff.round, handoff.role, handoff.verdict, handoff.next_role, handoff.summary
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_changes_say_what_was_put_back_and_what_lisa_must_check() {
        let text = hidden_changes_text(
            Role::Developer,
            &[PathBuf::from("/p/.git/hooks/pre-commit")],
            &[PathBuf::from("/home/lisa/.bashrc")],
        );

        assert!(text.contains("put back as they were:\n  /p/.git/hooks/pre-commit"));
        assert!(text.contains("NOT put back"));
        assert!(text.contains("\n  /home/lisa/.bashrc"));
        let only_hooks = hidden_changes_text(Role::Tester, &[PathBuf::from("h")], &[]);
        assert!(!only_hooks.contains("outside the project"));
    }
}
