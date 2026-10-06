//! `harness retro ...`: statistics of finished tasks, the retro agent's
//! proposals, and applying the ones Lisa picks.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use harness_agents::build::retro_agent;
use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::retro;
use harness_core::retro::proposals::FileChange;
use harness_core::retro::suggest::{self, Applied};

use crate::open_repo;

/// Statistics of one task, or of all tasks when `task_id` is `None`.
/// Printed, saved in `.harness/retros/<NNN>/` and committed. With `suggest`,
/// the `[retro]` agent then reads the history and proposes skill changes.
pub(crate) async fn retro(project: &Path, task_id: Option<&str>, suggest: bool) -> Result<()> {
    let repo = open_repo(project)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let config = match Config::load(&harness_dir) {
        Ok(config) => Some(config),
        Err(error) if !suggest => {
            eprintln!("Skills are not compared with harness.toml: {error}");
            None
        }
        Err(error) => return Err(error.into()),
    };
    // Check everything the agent needs before saving anything.
    let agent = match (&config, suggest) {
        (Some(config), true) => {
            retro::ops::check_clean(&repo)?;
            Some(retro_agent(config)?)
        }
        _ => None,
    };

    let (dir, stats) = retro::ops::save_stats(&repo, task_id, config.as_ref())?;
    print!("{}", stats.to_markdown());
    let number = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    println!("\nSaved to {}.", dir.display());

    let (Some(agent), Some(config)) = (agent, config) else {
        return Ok(());
    };
    println!("\nThe retro agent is reading the history...");
    let found = suggest::suggest(&repo, &dir, &stats, &config, &agent, suggest::ENGLISH)
        .await
        .with_context(|| {
            format!(
                "the agent's log is in {}",
                dir.join(suggest::AGENT_LOG).display()
            )
        })?;
    println!("\n{}", found.retro.trim_end());
    if found.proposals.proposals.is_empty() {
        println!("\nNo proposals.");
    } else {
        println!("\nProposals:");
        for proposal in &found.proposals.proposals {
            println!("  {}. {}", proposal.id, proposal.summary);
        }
        println!(
            "\nSee the changes with `harness retro show {number}`, \
             apply the ones you want with `harness retro apply {number} <ids>`."
        );
    }
    Ok(())
}

/// `.harness/retros/<NNN>` for `4`, `04` or `004`; it must exist.
fn retro_dir(repo: &Repo, number: &str) -> Result<PathBuf> {
    Ok(retro::ops::dir(repo, number)?)
}

/// `harness retro show <number>`: the retro notes and each proposal with its diff.
pub(crate) fn retro_show(project: &Path, number: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let dir = retro_dir(&repo, number)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let found = suggest::load(&dir).with_context(|| {
        format!(
            "retrospective {number} has no proposals; they come from `harness retro <task> --suggest`"
        )
    })?;
    let config = Config::load(&harness_dir)?;
    let applied = Applied::load(&dir);
    let mut text = format!("{}\n", found.retro.trim_end());
    if found.proposals.proposals.is_empty() {
        text.push_str("\nNo proposals.\n");
    }
    for proposal in &found.proposals.proposals {
        let mark = if applied.applied.contains(&proposal.id) {
            " [applied]"
        } else {
            ""
        };
        // Writing into a `String` cannot fail, so `let _ =` ignores the `Result`.
        let _ = write!(
            text,
            "\n{}.{mark} {}",
            proposal.id,
            proposal.describe(&harness_dir, &config)
        );
    }
    let _ = io::stdout().write_all(text.as_bytes());
    Ok(())
}

/// `harness retro apply <number> <ids>`: applies the chosen proposals to the
/// skills and `harness.toml`, then commits.
pub(crate) fn retro_apply(project: &Path, number: &str, ids: &[u32]) -> Result<()> {
    let repo = open_repo(project)?;
    let dir = retro_dir(&repo, number)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let found = suggest::load(&dir)?;
    let applied = Applied::load(&dir);
    // Say what will happen before it happens.
    for id in ids {
        if applied.applied.contains(id) {
            println!("Proposal {id} is already applied.");
        } else if let Some(proposal) = found.proposals.get(*id) {
            let file = match proposal.file_change(&harness_dir) {
                FileChange::New => " (new skill file)",
                FileChange::Changed { .. } => " (skill file changed)",
                FileChange::Unchanged => "",
            };
            println!("{id}. {}{file}", proposal.summary);
        }
    }
    if !retro::ops::apply(&repo, &dir, ids)?.is_empty() {
        println!("Applied and committed.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use harness_core::task::handoff::Role;

    use crate::init;
    use crate::tasks::new_task;

    #[tokio::test(flavor = "current_thread")]
    async fn retro_is_saved_and_committed() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        assert!(retro(dir.path(), None, false).await.is_err()); // no tasks yet
        init(dir.path()).unwrap();
        new_task(dir.path(), "task-001", "Build a parser", false).unwrap();

        retro(dir.path(), Some("task-001"), false).await.unwrap();
        retro(dir.path(), None, false).await.unwrap();
        assert!(retro(dir.path(), Some("task-404"), false).await.is_err());

        let retros = dir.path().join(".harness/retros");
        assert!(retros.join("001/stats.md").exists());
        let json = fs::read_to_string(retros.join("002/stats.json")).unwrap();
        assert!(json.contains("\"scope\": \"all\""), "{json}");
        assert!(repo.changed_files().unwrap().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retro_proposals_are_shown_and_applied() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        init(dir.path()).unwrap();
        new_task(dir.path(), "task-001", "Build a parser", false).unwrap();
        retro(dir.path(), Some("task-001"), false).await.unwrap();
        assert!(retro_show(dir.path(), "1").is_err(), "no proposals yet");

        // What `retro --suggest` would have saved.
        let saved = dir.path().join(".harness/retros/001");
        fs::write(saved.join("retro.md"), "Went well.").unwrap();
        fs::write(
            saved.join("proposals.json"),
            r#"{"proposals": [{"id": 1, "summary": "Check empty input", "reason": "r",
                "skill": "empty-input",
                "content": "---\ndescription: Check empty input.\n---\nTest it.\n",
                "roles": [{"role": "developer", "list": "skills"}]}]}"#,
        )
        .unwrap();
        repo.commit_all("saved proposals").unwrap();

        retro_show(dir.path(), "001").unwrap();
        assert!(retro_show(dir.path(), "9").is_err());
        assert!(retro_apply(dir.path(), "1", &[2]).is_err());

        retro_apply(dir.path(), "1", &[1]).unwrap();
        assert!(repo.changed_files().unwrap().is_empty());
        let harness = dir.path().join(".harness");
        let config = Config::load(&harness).unwrap();
        assert_eq!(config.roles[&Role::Developer].skills, ["empty-input"]);
        assert!(harness.join("skills/empty-input.md").exists());
        assert_eq!(Applied::load(&saved).applied, [1]);
        // A second time it only says so.
        retro_apply(dir.path(), "1", &[1]).unwrap();
    }
}
