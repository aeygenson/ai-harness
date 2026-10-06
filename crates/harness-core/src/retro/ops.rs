//! Retrospectives as `harness retro` and the TUI's «Retro» tab make them.
//!
//! The steps are the same for both: count the statistics and commit them
//! (`save_stats`), let the `[retro]` agent read the history
//! (`suggest::suggest`), and later apply the proposals Lisa picks
//! (`apply`). `list` reads what was saved, newest first.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::git::{GitError, Repo, HARNESS_DIR};
use crate::retro::proposals::ProposalsFile;
use crate::retro::suggest::{self, Applied, ApplyError, PROPOSALS_JSON, RETRO_MD};
use crate::retro::{RetroError, Scope, Stats, TaskHistory, RETROS_DIR};
use crate::task::store::StoreError;

const STATS_MD: &str = "stats.md";
const STATS_JSON: &str = "stats.json";

/// What can go wrong while saving, listing or applying a retrospective.
#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    /// The project has no tasks to count.
    #[error("there are no tasks yet; a retrospective is made from their history")]
    NoTasks,
    /// The task id given does not exist or cannot be read.
    #[error("cannot open task {0}: {1}")]
    NoTask(String, StoreError),
    /// The text given is not a number such as `004`.
    #[error("{0:?} is not a retrospective number, such as 004")]
    BadNumber(String),
    /// There is no `retros/<NNN>/` folder with this number.
    #[error("there is no retrospective {0}")]
    NoRetro(String),
    /// The retrospective's `retro.md` or `proposals.json` is missing or unreadable.
    #[error("retrospective {0} has no proposals")]
    NoProposals(String),
    /// Files that must be committed first have uncommitted changes.
    #[error("these files have uncommitted changes; commit or remove them first: {}", .0.join(", "))]
    Dirty(Vec<String>),
    /// The task history could not be read.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The statistics could not be saved.
    #[error(transparent)]
    Retro(#[from] RetroError),
    /// Running the Retrospective agent failed.
    #[error(transparent)]
    Suggest(#[from] suggest::SuggestError),
    /// A proposal could not be applied.
    #[error(transparent)]
    Apply(#[from] ApplyError),
    /// A git command failed.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// A saved retrospective, as the «Retro» tab shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroInfo {
    /// `003`.
    pub number: String,
    /// The folder `.harness/retros/<NNN>`.
    pub dir: PathBuf,
    /// What it looked at; `None` when `stats.json` cannot be read.
    pub scope: Option<Scope>,
    /// The day it was last committed, `2026-10-02`.
    pub date: Option<String>,
    /// What the agent wrote (and Lisa may have edited), if it ran.
    pub retro: Option<String>,
    /// The text of `stats.md`; `None` when it cannot be read.
    pub stats: Option<String>,
    /// The proposals, or why they cannot be read; `None` without any.
    pub proposals: Option<Result<ProposalsFile, String>>,
    /// The ids of the proposals already applied.
    pub applied: Vec<u32>,
}

/// Every saved retrospective, newest first.
pub fn list(repo: &Repo) -> Vec<RetroInfo> {
    let retros = repo.root().join(HARNESS_DIR).join(RETROS_DIR);
    let mut numbers: Vec<(u32, PathBuf)> = fs::read_dir(&retros)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let n = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
            Some((n, entry.path()))
        })
        .collect();
    numbers.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    numbers
        .into_iter()
        .map(|(n, dir)| info(repo, n, dir))
        .collect()
}

fn info(repo: &Repo, n: u32, dir: PathBuf) -> RetroInfo {
    let number = format!("{n:03}");
    let read = |name: &str| fs::read_to_string(dir.join(name)).ok();
    let scope = read(STATS_JSON)
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| json.get("scope")?.as_str().map(Scope::from_text));
    let proposals = read(PROPOSALS_JSON)
        .map(|text| serde_json::from_str::<ProposalsFile>(&text).map_err(|e| e.to_string()));
    RetroInfo {
        date: repo.last_change_date(&format!("{HARNESS_DIR}/{RETROS_DIR}/{number}")),
        retro: read(RETRO_MD),
        stats: read(STATS_MD),
        applied: Applied::load(&dir).applied,
        number,
        dir,
        scope,
        proposals,
    }
}

/// `.harness/retros/<NNN>` for `4`, `04` or `004`; it must exist.
pub fn dir(repo: &Repo, number: &str) -> Result<PathBuf, OpsError> {
    let n: u32 = number
        .parse()
        .map_err(|_| OpsError::BadNumber(number.to_string()))?;
    let dir = repo
        .root()
        .join(HARNESS_DIR)
        .join(RETROS_DIR)
        .join(format!("{n:03}"));
    if !dir.is_dir() {
        return Err(OpsError::NoRetro(format!("{n:03}")));
    }
    Ok(dir)
}

/// Counts the statistics of one task (or of all with `None`), saves them in
/// a new `retros/<NNN>/` and commits it. `config` lets the statistics compare
/// the skills with harness.toml.
pub fn save_stats(
    repo: &Repo,
    task_id: Option<&str>,
    config: Option<&Config>,
) -> Result<(PathBuf, Stats), OpsError> {
    let runs = repo.runs_dir();
    let (scope, tasks) = match task_id {
        Some(id) => (
            Scope::Task(id.to_string()),
            vec![TaskHistory::load(&runs, id).map_err(|e| OpsError::NoTask(id.to_string(), e))?],
        ),
        None => (Scope::All, TaskHistory::load_all(&runs)?),
    };
    if tasks.is_empty() {
        return Err(OpsError::NoTasks);
    }
    let stats = Stats::collect(scope, &tasks, config);
    let dir = stats.save(&repo.root().join(HARNESS_DIR))?;
    let number = dir.file_name().unwrap_or_default().to_string_lossy();
    let message = format!("harness: retro {number} ({})", stats.scope);
    repo.commit_paths(&[&dir], &message)?;
    Ok((dir, stats))
}

/// The project must have no uncommitted changes before the agent runs.
pub fn check_clean(repo: &Repo) -> Result<(), OpsError> {
    let dirty = repo.changed_files()?;
    if dirty.is_empty() {
        Ok(())
    } else {
        Err(OpsError::Dirty(dirty))
    }
}

/// Applies the proposals `ids` of the retrospective in `dir`: skill files,
/// the roles' skills in harness.toml, `applied.json`, then one commit.
/// Proposals applied before are skipped; returns the ids applied now.
pub fn apply(repo: &Repo, dir: &Path, ids: &[u32]) -> Result<Vec<u32>, OpsError> {
    let harness_dir = repo.root().join(HARNESS_DIR);
    // Only the harness's own changes may go into the commit.
    let dirty: Vec<String> = repo
        .changed_files()?
        .into_iter()
        .filter(|path| {
            path == ".harness/harness.toml"
                || path.starts_with(".harness/skills/")
                || path.starts_with(&format!(".harness/{RETROS_DIR}/"))
        })
        .collect();
    if !dirty.is_empty() {
        return Err(OpsError::Dirty(dirty));
    }
    let number = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let found = suggest::load(dir).map_err(|_| OpsError::NoProposals(number.clone()))?;
    let applied = Applied::load(dir);
    let mut chosen: Vec<u32> = Vec::new();
    for &id in ids {
        if !applied.applied.contains(&id) && !chosen.contains(&id) {
            chosen.push(id);
        }
    }
    if chosen.is_empty() {
        return Ok(chosen);
    }
    let mut paths = suggest::apply(&harness_dir, &found.proposals, &chosen)?;
    paths.push(Applied::add(dir, &chosen)?);
    let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let list: Vec<String> = chosen.iter().map(u32::to_string).collect();
    repo.commit_paths(
        &refs,
        &format!("harness: retro {number}, apply {}", list.join(", ")),
    )?;
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CONFIG_FILE;
    use crate::retro::proposals::{Proposal, RoleSkill, SkillList};
    use crate::task::handoff::Role;

    fn project() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let harness = dir.path().join(HARNESS_DIR);
        fs::create_dir_all(&harness).unwrap();
        fs::write(
            harness.join(CONFIG_FILE),
            "[roles.developer]\nagent = \"claude\"\n",
        )
        .unwrap();
        repo.commit_all("settings").unwrap();
        (dir, repo)
    }

    #[test]
    fn retrospectives_are_saved_listed_and_applied() {
        let (_dir, repo) = project();
        assert!(matches!(
            save_stats(&repo, None, None),
            Err(OpsError::NoTasks)
        ));
        assert!(list(&repo).is_empty());

        crate::task::orchestrator::create_task(&repo, "task-001", "Build a parser", 5).unwrap();
        assert!(matches!(
            save_stats(&repo, Some("task-404"), None),
            Err(OpsError::NoTask(..))
        ));
        let (first, _) = save_stats(&repo, Some("task-001"), None).unwrap();
        let (second, stats) = save_stats(&repo, None, None).unwrap();
        assert_eq!(stats.scope, Scope::All);
        check_clean(&repo).unwrap();

        // What the agent would have written into the second one.
        let file = ProposalsFile {
            proposals: vec![Proposal {
                id: 1,
                summary: "Teach the developer to check empty input".into(),
                reason: "task-001".into(),
                skill: "empty-input".into(),
                content: Some("---\ndescription: Check empty input.\n---\nCheck it.\n".into()),
                roles: vec![RoleSkill {
                    role: Role::Developer,
                    list: SkillList::Skills,
                }],
            }],
        };
        fs::write(second.join(RETRO_MD), "Went well.").unwrap();
        fs::write(
            second.join(PROPOSALS_JSON),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
        repo.commit_all("the agent's answer").unwrap();

        let found = list(&repo);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].number, "002");
        assert_eq!(found[0].scope, Some(Scope::All));
        assert_eq!(found[1].scope, Some(Scope::Task("task-001".into())));
        assert!(found[0].date.is_some());
        assert_eq!(found[0].retro.as_deref(), Some("Went well."));
        assert!(found[0]
            .stats
            .as_deref()
            .unwrap()
            .contains("# Retrospective"));
        assert_eq!(found[0].proposals, Some(Ok(file)));
        assert_eq!(found[1].proposals, None);

        assert_eq!(dir(&repo, "2").unwrap(), second);
        assert!(matches!(dir(&repo, "x"), Err(OpsError::BadNumber(_))));
        assert!(matches!(dir(&repo, "9"), Err(OpsError::NoRetro(_))));
        assert!(matches!(
            apply(&repo, &first, &[1]),
            Err(OpsError::NoProposals(_))
        ));

        assert_eq!(apply(&repo, &second, &[1]).unwrap(), [1]);
        assert!(repo.changed_files().unwrap().is_empty(), "committed");
        let skill = repo.root().join(".harness/skills/empty-input.md");
        assert!(skill.is_file());
        let toml = fs::read_to_string(repo.root().join(".harness/harness.toml")).unwrap();
        assert!(toml.contains("empty-input"), "{toml}");
        assert_eq!(list(&repo)[0].applied, [1]);
        // Applied once only.
        assert!(apply(&repo, &second, &[1]).unwrap().is_empty());
    }
}
