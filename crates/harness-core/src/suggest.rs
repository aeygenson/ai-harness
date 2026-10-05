//! The Retrospective: an AI agent reads the history and proposes skill changes.
//!
//! `harness retro <task> --suggest` first saves the statistics (see
//! `crate::retro`), then runs the `[retro]` agent. The agent may read the whole
//! project, but writes only into `retros/<NNN>/inbox/`: the harness checks this
//! with git, just as it does for the roles. It gets the read-only rules of the
//! security role, and no MCP servers, plugins or skills of its own.
//!
//! The agent writes `retro.md` (for Lisa) and `proposals.json` (see
//! `crate::proposals`). Nothing changes until Lisa picks proposals with
//! `harness retro apply <NNN> <ids>`; applied ids are kept in `applied.json`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::{AgentRunner, RoleJob};
use crate::config::{Config, ConfigError, CONFIG_FILE};
use crate::config_edit::{self, EditError};
use crate::git::{GitError, Repo, HARNESS_DIR};
use crate::handoff::Role;
use crate::proposals::{FileChange, ProposalError, ProposalsFile, SkillList};
use crate::retro::Stats;
use crate::skills::{SkillError, Skills, SKILLS_DIR};
use crate::text;

pub const RETRO_MD: &str = "retro.md";
pub const PROPOSALS_JSON: &str = "proposals.json";
pub const APPLIED_JSON: &str = "applied.json";
pub const AGENT_LOG: &str = "agent.log";
const INBOX_DIR: &str = "inbox";
/// The language the CLI asks for.
pub const ENGLISH: &str = "English";

/// The agent runs with the rules of this role: read everything, write only
/// its own output folder.
pub const RULES_OF: Role = Role::Security;

#[derive(Debug, thiserror::Error)]
pub enum SuggestError {
    #[error("the project has uncommitted changes; commit or remove them first: {}", .0.join(", "))]
    Dirty(Vec<String>),
    #[error("the retro agent made a git commit itself, which agents must never do")]
    AgentCommitted,
    #[error("the retro agent changed files it may not touch (left as they are): {}", .0.join(", "))]
    ForbiddenChanges(Vec<String>),
    #[error("the retro agent failed: {0}")]
    AgentFailed(String),
    #[error("the retro agent did not write {0}")]
    Missing(&'static str),
    #[error(transparent)]
    Proposals(#[from] ProposalError),
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("there is no proposal {0}")]
    UnknownId(u32),
    #[error(transparent)]
    Proposal(#[from] ProposalError),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("after the change the skills do not load, so nothing was changed: {0}")]
    Skills(#[from] SkillError),
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// What the agent wrote, after the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestions {
    pub retro: String,
    pub proposals: ProposalsFile,
}

/// `applied.json`: which proposals of one retrospective Lisa applied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub applied: Vec<u32>,
}

/// harness.toml as it was when some tasks ran, if it differs from now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastSettings {
    pub tasks: Vec<String>,
    pub text: String,
}

/// For each task, harness.toml at the task's last commit; tasks with the same
/// settings are grouped, and settings equal to `current` are left out.
pub fn past_settings(repo: &Repo, stats: &Stats, current: &str) -> Vec<PastSettings> {
    let mut past: Vec<PastSettings> = Vec::new();
    for task in &stats.tasks {
        let folder = format!("{HARNESS_DIR}/runs/{}", task.task_id);
        let file = format!("{HARNESS_DIR}/{CONFIG_FILE}");
        let Some(text) = repo.file_at_last_change(&folder, &file) else {
            continue;
        };
        if text.trim_end() == current.trim_end() {
            continue;
        }
        match past.iter_mut().find(|p| p.text == text) {
            Some(same) => same.tasks.push(task.task_id.clone()),
            None => past.push(PastSettings {
                tasks: vec![task.task_id.clone()],
                text,
            }),
        }
    }
    past
}

/// The prompt for the Retrospective.
pub fn prompt(
    stats: &Stats,
    runs_dir: &Path,
    harness_dir: &Path,
    config: &Config,
    past: &[PastSettings],
    output_dir: &Path,
    language: &str,
) -> String {
    let tasks: Vec<&str> = stats.tasks.iter().map(|t| t.task_id.as_str()).collect();
    let mut text = format!(
        "You are the Retrospective of a team of AI roles: architect, developer, \
         tester and security; \"human\" is Lisa, who leads the team. Your job is to \
         learn from finished work and propose better skills for the roles.\n\n\
         Statistics the harness counted for {scope}:\n\n{stats}\n\
         The full history is in {runs}/<task>/ for {tasks}: task.md is the task; \
         round-NN/NN-<role>/ has each step's handoff.json, notes.md and agent.log; \
         failures/ has logs of failed attempts. Read the notes and handoffs to \
         find what went wrong and why.\n\n",
        scope = stats.scope,
        stats = stats.to_markdown(),
        runs = runs_dir.display(),
        tasks = tasks.join(", "),
    );

    let skills_dir = harness_dir.join(SKILLS_DIR);
    text.push_str(&format!(
        "Skills are files {dir}/<name>.md that start with\n---\n\
         description: one line about the skill\n---\n\
         followed by the instructions. Names use lowercase letters, digits and '-'.\n\
         A role gets a skill in harness.toml: in `skills` it sees the description and \
         reads the file when needed; in `always_skills` the whole file is in its prompt.\n\
         The roles now have:\n",
        dir = skills_dir.display()
    ));
    for (role, settings) in &config.roles {
        text.push_str(&format!(
            "- {}: skills = {:?}, always_skills = {:?}\n",
            role.as_str(),
            settings.skills,
            settings.always_skills
        ));
    }
    if past.is_empty() {
        text.push_str("The tasks ran with these same settings.\n");
    } else {
        text.push_str(
            "These are the current settings. They changed after some tasks ran, so \
             judge each task by the settings it ran with, not by the current ones:\n",
        );
        for settings in past {
            text.push_str(&format!(
                "harness.toml when {} finished:\n```toml\n{}\n```\n",
                settings.tasks.join(", "),
                settings.text.trim_end()
            ));
        }
    }
    let files = skill_files(&skills_dir);
    if files.is_empty() {
        text.push_str("There are no skill files yet.\n");
    } else {
        text.push_str(&format!(
            "Skill files: {}. Read a file before you propose to change it.\n",
            files.join(", ")
        ));
    }

    text.push_str(&format!(
        "\nYou may propose only skills: a new skill file, a new text for an existing \
         one, and giving a skill to a role. You cannot propose changes to role \
         prompts, permissions, agents, models, MCP servers or plugins; write such \
         ideas in retro.md. Every proposal must say what in the history it is based \
         on. Few good proposals are better than many; propose nothing if nothing is \
         needed. A skill should be short and concrete.\n\n\
         Do not change any file of the project. Write exactly two files into {out}:\n\
         - {RETRO_MD}: a short retrospective for Lisa: what went well, what went \
         badly, repeated problems, and ideas that are not skills.\n\
         - {PROPOSALS_JSON}: your proposals, exactly in this format (JSON, these \
         fields only):\n{example}\n\
         Fields: id (1, 2, ...), summary (one line: what changes), reason (what in \
         the history it is based on), skill (the skill name), content (optional: \
         the whole new text of the skill file; leave it out to keep the file as it \
         is), roles (optional: roles that get the skill, list \"skills\" or \
         \"always_skills\"). Use {{\"proposals\": []}} if you propose nothing.\n\
         Write retro.md, and the summary and reason of each proposal, in \
         {language}; skill files stay in English.\n\
         Do not commit to git; the harness does that.\n",
        out = output_dir.display(),
        example = EXAMPLE,
    ));
    text
}

const EXAMPLE: &str = r#"{
  "proposals": [
    {
      "id": 1,
      "summary": "Teach the developer to handle empty input",
      "reason": "The tester rejected task-003 twice because the parser panicked on empty input.",
      "skill": "empty-input",
      "content": "---\ndescription: Check empty and missing input before using it.\n---\nEvery function that parses input must handle an empty string...\n",
      "roles": [{ "role": "developer", "list": "skills" }]
    }
  ]
}"#;

/// Runs the Retrospective for the statistics already saved and committed in
/// `retro_dir`, then saves and commits what it wrote.
pub async fn suggest<A: AgentRunner>(
    repo: &Repo,
    retro_dir: &Path,
    stats: &Stats,
    config: &Config,
    agent: &A,
    language: &str,
) -> Result<Suggestions, SuggestError> {
    repo.ensure_harness_ignores()?;
    let dirty = repo.changed_files()?;
    if !dirty.is_empty() {
        return Err(SuggestError::Dirty(dirty));
    }
    let harness_dir = retro_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(repo.root())
        .to_path_buf();
    let inbox = retro_dir.join(INBOX_DIR);
    if inbox.exists() {
        fs::remove_dir_all(&inbox).map_err(|e| io_error(&inbox, e))?;
    }
    fs::create_dir_all(&inbox).map_err(|e| io_error(&inbox, e))?;

    let number = retro_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let job = RoleJob {
        task_id: format!("retro-{number}"),
        round: 1,
        role: RULES_OF,
        project_dir: repo.root().to_path_buf(),
        prompt: {
            let current = fs::read_to_string(harness_dir.join(CONFIG_FILE)).unwrap_or_default();
            let past = past_settings(repo, stats, &current);
            prompt(
                stats,
                &repo.runs_dir(),
                &harness_dir,
                config,
                &past,
                &inbox,
                language,
            )
        },
        output_dir: inbox.clone(),
    };
    let head = repo.head()?;
    let outcome = agent.run(&job).await;

    let committed = repo.head()? != head;
    let changed = repo.changed_files()?;
    let log = retro_dir.join(AGENT_LOG);
    write(&log, &outcome.log)?;
    if committed {
        return Err(SuggestError::AgentCommitted);
    }
    if !changed.is_empty() {
        return Err(SuggestError::ForbiddenChanges(changed));
    }
    let failed = |error: SuggestError| -> Result<Suggestions, SuggestError> {
        repo.commit_paths(
            &[&log],
            &format!("harness: retro {number}, the agent failed"),
        )?;
        Err(error)
    };
    if !outcome.success {
        let message = if outcome.usage_limit_reached {
            "the usage limit was reached".to_string()
        } else {
            text::safe_line(&outcome.message, 500)
        };
        return failed(SuggestError::AgentFailed(message));
    }
    let Ok(retro) = fs::read_to_string(inbox.join(RETRO_MD)) else {
        return failed(SuggestError::Missing(RETRO_MD));
    };
    let retro = text::safe(&retro);
    let Ok(json) = fs::read_to_string(inbox.join(PROPOSALS_JSON)) else {
        return failed(SuggestError::Missing(PROPOSALS_JSON));
    };
    let proposals = match ProposalsFile::parse(&json, &harness_dir, config) {
        Ok(proposals) => proposals,
        Err(error) => {
            // Keep what the agent wrote, so Lisa can see what was wrong.
            write(&retro_dir.join("proposals.rejected.json"), &json)?;
            let rejected = retro_dir.join("proposals.rejected.json");
            repo.commit_paths(
                &[&log, &rejected],
                &format!("harness: retro {number}, the proposals were refused"),
            )?;
            return Err(error.into());
        }
    };

    let md = retro_dir.join(RETRO_MD);
    write(&md, &retro)?;
    let saved = retro_dir.join(PROPOSALS_JSON);
    let pretty = serde_json::to_string_pretty(&proposals).unwrap_or_default();
    write(&saved, &pretty)?;
    fs::remove_dir_all(&inbox).map_err(|e| io_error(&inbox, e))?;
    repo.commit_paths(
        &[&log, &md, &saved],
        &format!(
            "harness: retro {number}, {} proposals",
            proposals.proposals.len()
        ),
    )?;
    Ok(Suggestions { retro, proposals })
}

/// Reads `retro.md` and `proposals.json` of a saved retrospective.
pub fn load(retro_dir: &Path) -> Result<Suggestions, SuggestError> {
    let retro = fs::read_to_string(retro_dir.join(RETRO_MD))
        .map_err(|e| io_error(&retro_dir.join(RETRO_MD), e))?;
    let path = retro_dir.join(PROPOSALS_JSON);
    let json = fs::read_to_string(&path).map_err(|e| io_error(&path, e))?;
    let mut proposals: ProposalsFile =
        serde_json::from_str(&json).map_err(|e| ProposalError::Format(e.to_string()))?;
    // Saved files are cleaned on the way in, but older ones may not be.
    proposals.make_text_safe();
    Ok(Suggestions {
        retro: text::safe(&retro),
        proposals,
    })
}

impl Applied {
    pub fn load(retro_dir: &Path) -> Self {
        fs::read_to_string(retro_dir.join(APPLIED_JSON))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Adds `ids` and saves the file; returns its path.
    pub fn add(retro_dir: &Path, ids: &[u32]) -> Result<PathBuf, ApplyError> {
        let mut applied = Self::load(retro_dir);
        for id in ids {
            if !applied.applied.contains(id) {
                applied.applied.push(*id);
            }
        }
        applied.applied.sort_unstable();
        let path = retro_dir.join(APPLIED_JSON);
        let text = serde_json::to_string_pretty(&applied).unwrap_or_default();
        fs::write(&path, text).map_err(|source| ApplyError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }
}

/// Applies the chosen proposals: writes the skill files and adds the skills to
/// the roles in harness.toml. If the result does not load, everything is put
/// back. Returns the changed files.
pub fn apply(
    harness_dir: &Path,
    proposals: &ProposalsFile,
    ids: &[u32],
) -> Result<Vec<PathBuf>, ApplyError> {
    let config_path = harness_dir.join(CONFIG_FILE);
    let old_config = fs::read_to_string(&config_path).map_err(|source| ApplyError::Io {
        path: config_path.clone(),
        source,
    })?;
    let config = Config::load(harness_dir)?;

    // Check everything before changing anything.
    let mut chosen = Vec::new();
    for &id in ids {
        let proposal = proposals.get(id).ok_or(ApplyError::UnknownId(id))?;
        proposal.check(harness_dir, &config)?;
        chosen.push(proposal);
    }
    let mut text = old_config.clone();
    for proposal in &chosen {
        for given in proposal.missing_roles(&config) {
            let always = given.list == SkillList::AlwaysSkills;
            text = config_edit::add_role_skill(&text, given.role, &proposal.skill, always)?;
        }
    }

    // (path, old text or None if the file is new), to undo a failed change.
    let mut backups: Vec<(PathBuf, Option<String>)> = Vec::new();
    let mut result = Ok(());
    for proposal in &chosen {
        let path = proposal.skill_path(harness_dir);
        let (FileChange::New | FileChange::Changed { .. }, Some(content)) =
            (proposal.file_change(harness_dir), &proposal.content)
        else {
            continue;
        };
        if backups.iter().any(|(p, _)| p == &path) {
            // Two chosen proposals write the same file: the later one wins.
        } else {
            backups.push((path.clone(), fs::read_to_string(&path).ok()));
        }
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, content));
        if let Err(source) = written {
            result = Err(ApplyError::Io { path, source });
            break;
        }
    }
    if result.is_ok() && text != old_config {
        backups.push((config_path.clone(), Some(old_config)));
        if let Err(source) = fs::write(&config_path, &text) {
            result = Err(ApplyError::Io {
                path: config_path.clone(),
                source,
            });
        }
    }
    if result.is_ok() {
        result = Config::load(harness_dir)
            .map_err(ApplyError::from)
            .and_then(|config| Skills::load(harness_dir, &config).map_err(ApplyError::from))
            .map(|_| ());
    }
    if let Err(error) = result {
        for (path, old) in backups {
            let _ = match old {
                Some(text) => fs::write(&path, text),
                None => fs::remove_file(&path),
            };
        }
        return Err(error);
    }
    Ok(backups.into_iter().map(|(path, _)| path).collect())
}

/// The names of the skill files, without `.md`, sorted.
fn skill_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".md").map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

fn write(path: &Path, text: &str) -> Result<(), SuggestError> {
    fs::write(path, text).map_err(|e| io_error(path, e))
}

fn io_error(path: &Path, source: io::Error) -> SuggestError {
    SuggestError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentOutcome;
    use crate::proposals::{Proposal, RoleSkill};
    use crate::retro::TaskHistory;
    use std::future::Future;

    const TOML: &str = "[roles.developer]\nagent = \"claude\"\nskills = [\"style\"]\n\
                        [roles.tester]\nagent = \"codex\"\n";
    const STYLE: &str = "---\ndescription: Style.\n---\nOld rule.\n";
    const NEW_SKILL: &str = "---\ndescription: Check empty input.\n---\nTest \"\" first.\n";

    /// A project with one committed task and a committed first retrospective.
    fn project() -> (tempfile::TempDir, Repo, PathBuf, Stats, Config) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let harness = dir.path().join(".harness");
        fs::create_dir_all(harness.join(SKILLS_DIR)).unwrap();
        fs::write(harness.join(CONFIG_FILE), TOML).unwrap();
        fs::write(harness.join("skills/style.md"), STYLE).unwrap();
        repo.commit_all("settings").unwrap();
        crate::orchestrator::create_task(&repo, "task-001", "Build a parser", 5).unwrap();
        let config = Config::load(&harness).unwrap();
        let tasks = TaskHistory::load_all(&repo.runs_dir()).unwrap();
        let stats = Stats::collect("all", &tasks, Some(&config));
        let retro_dir = stats.save(&harness).unwrap();
        repo.commit_all("setup").unwrap();
        (dir, repo, retro_dir, stats, config)
    }

    /// An agent that writes the given files into its output folder, and maybe more.
    struct FakeAgent {
        files: Vec<(&'static str, String)>,
        success: bool,
        also_write: Option<&'static str>,
    }

    impl FakeAgent {
        fn writing(retro: &str, proposals: &str) -> Self {
            Self {
                files: vec![
                    (RETRO_MD, retro.to_string()),
                    (PROPOSALS_JSON, proposals.to_string()),
                ],
                success: true,
                also_write: None,
            }
        }
    }

    impl AgentRunner for FakeAgent {
        fn run(&self, job: &RoleJob) -> impl Future<Output = AgentOutcome> + Send {
            assert_eq!(job.role, RULES_OF);
            assert!(job.prompt.contains("You may propose only skills"));
            for (name, text) in &self.files {
                fs::write(job.output_dir.join(name), text).unwrap();
            }
            if let Some(path) = self.also_write {
                fs::write(job.project_dir.join(path), "sneaky").unwrap();
            }
            let outcome = AgentOutcome {
                success: self.success,
                log: "agent talked".into(),
                message: if self.success { "" } else { "timed out" }.into(),
                ..AgentOutcome::default()
            };
            async move { outcome }
        }
    }

    fn proposals_json() -> String {
        let file = ProposalsFile {
            proposals: vec![
                Proposal {
                    id: 1,
                    summary: "New skill".into(),
                    reason: "Tester found panics".into(),
                    skill: "empty-input".into(),
                    content: Some(NEW_SKILL.into()),
                    roles: vec![RoleSkill {
                        role: Role::Developer,
                        list: SkillList::Skills,
                    }],
                },
                Proposal {
                    id: 2,
                    summary: "Style for the tester".into(),
                    reason: "Tester wrote messy tests".into(),
                    skill: "style".into(),
                    content: None,
                    roles: vec![RoleSkill {
                        role: Role::Tester,
                        list: SkillList::AlwaysSkills,
                    }],
                },
            ],
        };
        serde_json::to_string(&file).unwrap()
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn the_prompt_has_the_stats_the_history_and_the_skills() {
        let (_dir, repo, retro_dir, stats, config) = project();
        let harness = repo.root().join(".harness");
        let current = fs::read_to_string(harness.join(CONFIG_FILE)).unwrap();
        assert!(past_settings(&repo, &stats, &current).is_empty());
        let text = prompt(
            &stats,
            &repo.runs_dir(),
            &harness,
            &config,
            &[],
            &retro_dir,
            "Russian",
        );
        assert!(
            text.contains("The tasks ran with these same settings."),
            "{text}"
        );
        for part in [
            "# Retrospective: all",
            "for task-001",
            "- developer: skills = [\"style\"], always_skills = []",
            "Skill files: style.",
            "You cannot propose changes to role prompts, permissions",
            "\"list\": \"skills\"",
            "in Russian; skill files stay in English",
        ] {
            assert!(text.contains(part), "missing {part:?} in:\n{text}");
        }
    }

    #[test]
    fn settings_changed_after_a_task_are_shown_as_they_were() {
        let (_dir, repo, retro_dir, stats, config) = project();
        let harness = repo.root().join(".harness");
        let old = fs::read_to_string(harness.join(CONFIG_FILE)).unwrap();
        let new = old.replace("skills = [\"style\"]\n", "");
        fs::write(harness.join(CONFIG_FILE), &new).unwrap();
        repo.commit_all("the developer loses style").unwrap();

        let past = past_settings(&repo, &stats, &new);
        assert_eq!(
            past,
            [PastSettings {
                tasks: vec!["task-001".into()],
                text: old.clone()
            }]
        );
        let text = prompt(
            &stats,
            &repo.runs_dir(),
            &harness,
            &config,
            &past,
            &retro_dir,
            ENGLISH,
        );
        assert!(
            text.contains("judge each task by the settings it ran with"),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "harness.toml when task-001 finished:\n```toml\n{}",
                old.trim_end()
            )),
            "{text}"
        );
    }

    #[test]
    fn suggestions_are_checked_saved_and_committed() {
        let (_dir, repo, retro_dir, stats, config) = project();
        let agent = FakeAgent::writing("Went well.", &proposals_json());
        let found = block_on(suggest(&repo, &retro_dir, &stats, &config, &agent, ENGLISH)).unwrap();
        assert_eq!(found.retro, "Went well.");
        assert_eq!(found.proposals.proposals.len(), 2);
        assert!(!retro_dir.join("inbox").exists());
        assert_eq!(load(&retro_dir).unwrap(), found);
        assert_eq!(
            fs::read_to_string(retro_dir.join(AGENT_LOG)).unwrap(),
            "agent talked"
        );
        assert!(repo.changed_files().unwrap().is_empty());
    }

    #[test]
    fn a_failed_or_wrong_agent_changes_nothing_else() {
        let (_dir, repo, retro_dir, stats, config) = project();
        let run = |agent: FakeAgent| {
            block_on(suggest(&repo, &retro_dir, &stats, &config, &agent, ENGLISH))
        };

        let mut failing = FakeAgent::writing("x", "{\"proposals\": []}");
        failing.success = false;
        assert!(matches!(run(failing), Err(SuggestError::AgentFailed(m)) if m == "timed out"));
        assert!(
            repo.changed_files().unwrap().is_empty(),
            "the log is committed"
        );

        let missing = FakeAgent {
            files: vec![(RETRO_MD, "x".into())],
            success: true,
            also_write: None,
        };
        assert!(matches!(
            run(missing),
            Err(SuggestError::Missing(PROPOSALS_JSON))
        ));

        let bad = FakeAgent::writing("x", r#"{"proposals": [{"id": 1}]}"#);
        assert!(matches!(run(bad), Err(SuggestError::Proposals(_))));
        assert!(retro_dir.join("proposals.rejected.json").exists());
        assert!(repo.changed_files().unwrap().is_empty());

        let mut sneaky = FakeAgent::writing("x", "{\"proposals\": []}");
        sneaky.also_write = Some(".harness/skills/style.md");
        match run(sneaky) {
            Err(SuggestError::ForbiddenChanges(files)) => {
                assert_eq!(files, [".harness/skills/style.md"]);
            }
            other => panic!("{other:?}"),
        }
        // The next run refuses to start until Lisa looks at the changes.
        let agent = FakeAgent::writing("x", "{\"proposals\": []}");
        assert!(matches!(run(agent), Err(SuggestError::Dirty(_))));
    }

    #[test]
    fn apply_writes_skills_and_roles_only_for_the_chosen_ids() {
        let (_dir, repo, retro_dir, stats, config) = project();
        let harness = repo.root().join(".harness");
        let agent = FakeAgent::writing("x", &proposals_json());
        let found = block_on(suggest(&repo, &retro_dir, &stats, &config, &agent, ENGLISH)).unwrap();

        assert!(matches!(
            apply(&harness, &found.proposals, &[7]),
            Err(ApplyError::UnknownId(7))
        ));
        let changed = apply(&harness, &found.proposals, &[1]).unwrap();
        assert_eq!(
            changed,
            [
                harness.join("skills/empty-input.md"),
                harness.join(CONFIG_FILE)
            ]
        );
        let config = Config::load(&harness).unwrap();
        assert_eq!(
            config.roles[&Role::Developer].skills,
            ["style", "empty-input"]
        );
        assert!(config.roles[&Role::Tester].always_skills.is_empty());
        assert_eq!(
            fs::read_to_string(harness.join("skills/empty-input.md")).unwrap(),
            NEW_SKILL
        );

        // Applying again changes nothing.
        assert!(apply(&harness, &found.proposals, &[1]).unwrap().is_empty());

        Applied::add(&retro_dir, &[2, 1]).unwrap();
        Applied::add(&retro_dir, &[1]).unwrap();
        assert_eq!(Applied::load(&retro_dir).applied, [1, 2]);
    }

    #[test]
    fn apply_puts_everything_back_if_the_result_does_not_load() {
        let (_dir, repo, _retro_dir, _stats, _config) = project();
        let harness = repo.root().join(".harness");
        // The tester already lists a skill whose file is missing, so loading fails.
        let broken = format!("{TOML}skills = [\"gone\"]\n");
        fs::write(harness.join(CONFIG_FILE), &broken).unwrap();
        let file: ProposalsFile = serde_json::from_str(&proposals_json()).unwrap();

        assert!(matches!(
            apply(&harness, &file, &[1, 2]),
            Err(ApplyError::Skills(_))
        ));
        assert_eq!(
            fs::read_to_string(harness.join(CONFIG_FILE)).unwrap(),
            broken
        );
        assert!(!harness.join("skills/empty-input.md").exists());
    }
}
