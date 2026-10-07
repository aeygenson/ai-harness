//! Retrospective statistics: what happened in one task or in all of them.
//!
//! Everything here is counted by our own code from the saved history
//! (`handoff.json` files and failed attempts), without any AI:
//!
//! - how many rounds each task took and where it stopped;
//! - what each role decided, and who sent work back to whom;
//! - which problems (`issues`) were found, and which came up more than once;
//! - which skills the roles used, and which were configured but never used;
//! - how many tokens each role used and what it cost (see `usage`).
//!
//! The result is saved in `.harness/retros/<NNN>/` as `stats.md` (for Lisa)
//! and `stats.json` (for programs, and later for the Retrospective role).
//!
//! The folder also holds `ops` (saving and applying retrospectives),
//! `suggest` (the Retrospective agent) and `proposals` (the changes it proposes).

pub mod ops;
pub mod proposals;
pub mod suggest;

pub mod usage;

mod collect;
mod markdown;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::task::handoff::{Handoff, Role, Severity};
use crate::task::store::{self, StoreError, TaskStore};
use crate::task::{Stage, TaskState, WaitReason};
use usage::Usage;

/// Folder inside `.harness/` where retrospectives are saved.
pub const RETROS_DIR: &str = "retros";
const STATS_MD: &str = "stats.md";
const STATS_JSON: &str = "stats.json";

/// The ways counting or saving a retrospective can fail.
#[derive(Debug, thiserror::Error)]
pub enum RetroError {
    /// The task history could not be read.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A file or folder of the retrospective could not be written.
    #[error("cannot write {path}: {source}")]
    Io {
        /// The file or folder that could not be written.
        path: PathBuf,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },
    /// The statistics could not be turned into JSON.
    #[error("cannot write stats.json: {0}")]
    Json(#[from] serde_json::Error),
}

/// One task as it is saved on disk.
#[derive(Debug, Clone)]
pub struct TaskHistory {
    /// The task's saved state: its id, current round, stage and round limit.
    pub state: TaskState,
    /// Every saved handoff of the task, in the order they happened.
    pub handoffs: Vec<Handoff>,
    /// Failed attempts: (round, role).
    pub failures: Vec<(u32, Role)>,
    /// Tokens and cost of each role's steps, from their `agent.log`; roles
    /// whose agent reports no usage are missing.
    pub usage: BTreeMap<Role, Usage>,
}

impl TaskHistory {
    /// Reads one task's state, handoffs and failed attempts from `runs_dir`.
    pub fn load(runs_dir: &Path, task_id: &str) -> Result<Self, StoreError> {
        let (store, state) = TaskStore::open(runs_dir, task_id)?;
        let steps = store.steps()?;
        let mut usage: BTreeMap<Role, Usage> = BTreeMap::new();
        for step in &steps {
            if let Some(found) = step.agent_log().as_deref().and_then(usage::from_log) {
                usage.entry(step.handoff.role).or_default().add(found);
            }
        }
        Ok(Self {
            state,
            handoffs: steps.into_iter().map(|step| step.handoff).collect(),
            failures: store.failures()?,
            usage,
        })
    }

    /// Every task in `runs_dir`, sorted by id.
    pub fn load_all(runs_dir: &Path) -> Result<Vec<Self>, StoreError> {
        store::task_ids(runs_dir)?
            .iter()
            .map(|id| Self::load(runs_dir, id))
            .collect()
    }
}

/// What a retrospective looked at: every task, or one.
//
// `into = "String"` makes serde write it as the plain text `"all"` or the
// task id, so `stats.json` looks the same as before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(into = "String")]
pub enum Scope {
    /// Every task of the project.
    All,
    /// One task, by its id (`task-001`).
    Task(String),
}

impl Scope {
    /// How [`Scope::All`] is written in `stats.json`.
    const ALL: &'static str = "all";

    /// Reads the text `stats.json` keeps back into a scope.
    pub fn from_text(text: &str) -> Self {
        if text == Self::ALL {
            Scope::All
        } else {
            Scope::Task(text.to_string())
        }
    }
}

/// Prints `all` or the task id, as in `stats.json`.
impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::All => f.write_str(Self::ALL),
            Scope::Task(id) => f.write_str(id),
        }
    }
}

/// Used by serde to write the scope into `stats.json`.
impl From<Scope> for String {
    fn from(scope: Scope) -> Self {
        scope.to_string()
    }
}

/// Everything the retrospective counted. Saved as `stats.json`.
//
// Not `Eq`: the cost in `usage` is a fraction (`f64`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    /// Which tasks were counted: all of them or one.
    pub scope: Scope,
    /// One summary per counted task.
    pub tasks: Vec<TaskSummary>,
    /// What each role did, summed over all counted tasks.
    pub roles: BTreeMap<Role, RoleStats>,
    /// Work sent back, most frequent first.
    pub returns: Vec<Return>,
    /// How many problems the handoffs listed, by severity.
    pub issues: SeverityCounts,
    /// Problems with the same description found more than once.
    pub repeated_issues: Vec<RepeatedIssue>,
    /// Empty when harness.toml could not be read.
    pub skills: Vec<SkillUse>,
    /// Tokens and cost of each role, summed over all counted tasks.
    pub usage: BTreeMap<Role, Usage>,
}

/// The numbers for one task, as one row of the task table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskSummary {
    /// The task id, such as `task-001`.
    pub task_id: String,
    /// The round the task reached, starting at 1.
    pub rounds: u32,
    /// The round limit of the task; past it the task waits for Lisa.
    pub max_rounds: u32,
    /// Where the task is now, e.g. `done` or `waiting: approve design`.
    pub stage: String,
    /// Saved handoffs, Lisa's decisions included.
    pub steps: usize,
    /// How many of those handoffs are Lisa's own decisions.
    pub human_decisions: usize,
    /// How many times a role's run failed in this task.
    pub failed_attempts: usize,
}

/// What one role did across the counted tasks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RoleStats {
    /// How many handoffs the role saved.
    pub steps: usize,
    /// Handoffs with the verdict `approved`.
    pub approved: usize,
    /// Handoffs with the verdict `rejected`.
    pub rejected: usize,
    /// Handoffs with the verdict `needs_human`: the role asked Lisa to decide.
    pub needs_human: usize,
    /// How many times the role's run failed.
    pub failed_attempts: usize,
    /// How many problems the role listed in the `issues` of its handoffs.
    pub issues_found: usize,
}

/// `from` rejected the work and sent it to `to`, `count` times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Return {
    /// The role that rejected the work.
    pub from: Role,
    /// The role the work was sent back to.
    pub to: Role,
    /// How many times this happened.
    pub count: usize,
}

/// How many problems were found at each severity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SeverityCounts {
    /// Problems marked `low`.
    pub low: usize,
    /// Problems marked `medium`.
    pub medium: usize,
    /// Problems marked `high`.
    pub high: usize,
    /// Problems marked `critical`.
    pub critical: usize,
}

impl SeverityCounts {
    fn add(&mut self, severity: Severity) {
        match severity {
            Severity::Low => self.low += 1,
            Severity::Medium => self.medium += 1,
            Severity::High => self.high += 1,
            Severity::Critical => self.critical += 1,
        }
    }

    /// All problems, whatever their severity.
    pub fn total(&self) -> usize {
        self.low + self.medium + self.high + self.critical
    }
}

/// A problem described the same way more than once (case and spaces ignored).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepeatedIssue {
    /// The description as it was first written.
    pub description: String,
    /// How many times it was found.
    pub count: usize,
    /// The highest severity it was given.
    pub severity: Severity,
    /// The roles that found it, in the order they first did.
    pub roles: Vec<Role>,
    /// The ids of the tasks it was found in.
    pub tasks: Vec<String>,
}

/// How a skill is given to a role in harness.toml.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSetting {
    /// In `skills`: the agent reads it when it needs it.
    OnDemand,
    /// In `always_skills`: always in the prompt.
    Always,
    /// `<plugin>:<skill>`, from a plugin in the role's `plugins`.
    Plugin,
    /// Reported in `skills_used`, but not given to this role (now).
    NotConfigured,
}

/// One skill of one role: how it is configured and how often it was used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillUse {
    /// The role the skill belongs to (or that reported using it).
    pub role: Role,
    /// The skill name; a plugin's skill is written `<plugin>:<skill>`.
    pub skill: String,
    /// How the skill is given to the role in harness.toml.
    pub setting: SkillSetting,
    /// In how many handoffs of this role the skill was listed in `skills_used`.
    /// The role reports this itself, so it is what the role says it used.
    pub used: usize,
}

impl Stats {
    /// Skills configured for a role but never listed in its `skills_used`.
    pub fn unused_skills(&self) -> impl Iterator<Item = &SkillUse> {
        self.skills
            .iter()
            .filter(|s| s.used == 0 && s.setting != SkillSetting::NotConfigured)
    }

    /// Saves `stats.md` and `stats.json` in a new `retros/<NNN>/` folder
    /// inside `harness_dir`, and returns that folder.
    pub fn save(&self, harness_dir: &Path) -> Result<PathBuf, RetroError> {
        let retros = harness_dir.join(RETROS_DIR);
        fs::create_dir_all(&retros).map_err(|e| io_error(&retros, e))?;
        let dir = retros.join(format!("{:03}", next_number(&retros)?));
        fs::create_dir(&dir).map_err(|e| io_error(&dir, e))?;
        let md = dir.join(STATS_MD);
        fs::write(&md, self.to_markdown()).map_err(|e| io_error(&md, e))?;
        let json = dir.join(STATS_JSON);
        let text = serde_json::to_string_pretty(self)?;
        fs::write(&json, text).map_err(|e| io_error(&json, e))?;
        Ok(dir)
    }
}

/// The next free number in `retros/`: one more than the highest `NNN` folder.
fn next_number(retros: &Path) -> Result<u32, RetroError> {
    let mut highest = 0;
    for entry in fs::read_dir(retros).map_err(|e| io_error(retros, e))? {
        let entry = entry.map_err(|e| io_error(retros, e))?;
        if let Ok(n) = entry.file_name().to_string_lossy().parse::<u32>() {
            highest = highest.max(n);
        }
    }
    Ok(highest + 1)
}

/// Where a task is, in words: `done`, `working: tester`, `waiting: approve design`.
pub fn stage_text(stage: Stage) -> String {
    match stage {
        Stage::Working(role) => format!("working: {}", role.as_str()),
        Stage::WaitingForHuman(WaitReason::ApproveDesign) => "waiting: approve design".into(),
        Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(role)) => {
            format!("waiting: {} asked for help", role.as_str())
        }
        Stage::WaitingForHuman(WaitReason::RoundLimitReached) => {
            "waiting: round limit reached".into()
        }
        Stage::Done => "done".into(),
    }
}

fn io_error(path: &Path, source: io::Error) -> RetroError {
    RetroError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::task::handoff::Issue;
    use crate::task::handoff::{NextStep, Verdict};

    fn handoff(role: Role, verdict: Verdict, next: NextStep) -> Handoff {
        Handoff {
            schema_version: 1,
            task_id: "task-001".into(),
            round: 1,
            role,
            verdict,
            next_role: next,
            summary: String::new(),
            skills_used: vec![],
            files: vec![],
            issues: vec![],
        }
    }

    fn issue(severity: Severity, description: &str) -> Issue {
        Issue {
            severity,
            location: None,
            description: description.into(),
        }
    }

    fn task(id: &str, stage: Stage, handoffs: Vec<Handoff>) -> TaskHistory {
        TaskHistory {
            state: TaskState {
                task_id: id.into(),
                round: 2,
                max_rounds: 5,
                stage,
            },
            handoffs,
            failures: vec![],
            usage: BTreeMap::new(),
        }
    }

    /// Architect -> Lisa -> developer -> tester rejects twice -> done.
    fn history() -> Vec<TaskHistory> {
        let mut architect = handoff(
            Role::Architect,
            Verdict::Approved,
            NextStep::To(Role::Human),
        );
        architect.skills_used = vec!["rust-design".into(), "rust-design".into()];
        let human = handoff(
            Role::Human,
            Verdict::Approved,
            NextStep::To(Role::Developer),
        );
        let mut developer = handoff(
            Role::Developer,
            Verdict::Approved,
            NextStep::To(Role::Tester),
        );
        developer.skills_used = vec!["surprise".into(), "review:check".into()];
        let mut tester = handoff(
            Role::Tester,
            Verdict::Rejected,
            NextStep::To(Role::Developer),
        );
        tester.issues = vec![issue(Severity::Medium, "Panics on empty input.")];
        let mut tester_again = tester.clone();
        tester_again.issues = vec![
            issue(Severity::High, "panics  on EMPTY input"),
            issue(Severity::Low, "typo in README"),
        ];
        let mut tester_ok = handoff(Role::Tester, Verdict::Approved, NextStep::Done);
        // The tester has no `review` plugin, but says it used its skill.
        tester_ok.skills_used = vec!["review:check".into()];

        let mut first = task(
            "task-001",
            Stage::Done,
            vec![architect, human, developer, tester, tester_again, tester_ok],
        );
        first.failures = vec![(1, Role::Developer)];

        let mut security = handoff(
            Role::Security,
            Verdict::Rejected,
            NextStep::To(Role::Developer),
        );
        security.issues = vec![issue(Severity::Critical, "Panics on empty input")];
        let second = task(
            "task-002",
            Stage::WaitingForHuman(WaitReason::ApproveDesign),
            vec![security],
        );
        vec![first, second]
    }

    fn config() -> Config {
        Config::parse(
            "[roles.architect]\nagent = \"claude\"\nskills = [\"rust-design\", \"never\"]\n\
             [roles.developer]\nagent = \"codex\"\nalways_skills = [\"errors\"]\n\
             plugins = [\"review\"]\n[plugins.review]\nagent = \"codex\"\n",
        )
        .unwrap()
    }

    #[test]
    fn counts_roles_returns_and_tasks() {
        let stats = Stats::collect(Scope::All, &history(), None);

        let tester = &stats.roles[&Role::Tester];
        assert_eq!((tester.steps, tester.approved, tester.rejected), (3, 1, 2));
        assert_eq!(tester.issues_found, 3);
        assert_eq!(stats.roles[&Role::Developer].failed_attempts, 1);
        assert_eq!(stats.roles[&Role::Human].steps, 1);

        assert_eq!(
            stats.returns,
            [
                Return {
                    from: Role::Tester,
                    to: Role::Developer,
                    count: 2
                },
                Return {
                    from: Role::Security,
                    to: Role::Developer,
                    count: 1
                },
            ]
        );

        assert_eq!(stats.tasks[0].steps, 6);
        assert_eq!(stats.tasks[0].human_decisions, 1);
        assert_eq!(stats.tasks[0].failed_attempts, 1);
        assert_eq!(stats.tasks[0].stage, "done");
        assert_eq!(stats.tasks[1].stage, "waiting: approve design");
        assert_eq!(stats.skills, Vec::<SkillUse>::new());
    }

    #[test]
    fn the_same_issue_is_found_across_roles_and_tasks() {
        let stats = Stats::collect(Scope::All, &history(), None);
        assert_eq!(
            stats.issues,
            SeverityCounts {
                low: 1,
                medium: 1,
                high: 1,
                critical: 1
            }
        );
        assert_eq!(
            stats.repeated_issues,
            [RepeatedIssue {
                description: "Panics on empty input.".into(),
                count: 3,
                severity: Severity::Critical,
                roles: vec![Role::Tester, Role::Security],
                tasks: vec!["task-001".into(), "task-002".into()],
            }]
        );
    }

    #[test]
    fn skills_show_configured_used_and_unknown() {
        let stats = Stats::collect(Scope::Task("task-001".into()), &history(), Some(&config()));
        let row = |role, skill: &str| {
            stats
                .skills
                .iter()
                .find(|s| s.role == role && s.skill == skill)
                .map(|s| (s.setting, s.used))
        };
        // Listed twice in one handoff: one use.
        assert_eq!(
            row(Role::Architect, "rust-design"),
            Some((SkillSetting::OnDemand, 1))
        );
        assert_eq!(
            row(Role::Architect, "never"),
            Some((SkillSetting::OnDemand, 0))
        );
        assert_eq!(
            row(Role::Developer, "errors"),
            Some((SkillSetting::Always, 0))
        );
        assert_eq!(
            row(Role::Developer, "surprise"),
            Some((SkillSetting::NotConfigured, 1))
        );
        assert_eq!(
            row(Role::Developer, "review:check"),
            Some((SkillSetting::Plugin, 1))
        );
        assert_eq!(
            row(Role::Tester, "review:check"),
            Some((SkillSetting::NotConfigured, 1))
        );
        let unused: Vec<&str> = stats.unused_skills().map(|s| s.skill.as_str()).collect();
        assert_eq!(unused, ["never", "errors"]);
    }

    #[test]
    fn markdown_has_every_section() {
        let md = Stats::collect(Scope::All, &history(), Some(&config())).to_markdown();
        for part in [
            "# Retrospective: all",
            "| task-001 | 2 of 5 | done | 6 | 1 | 1 |",
            "| tester | 3 | 1 | 2 | 0 | 0 | 3 |",
            "- tester -> developer: 2 times",
            "4 in total: critical 1, high 1, medium 1, low 1.",
            "- 3 times, critical: Panics on empty input. (by tester, security; in task-001, task-002)",
            "| Role | Skill | Setting | Listed in skills_used |",
            "| developer | review:check | plugin | 1 |",
            "| developer | surprise | not configured | 1 |",
            "Configured but never used: never (architect), errors (developer).",
        ] {
            assert!(md.contains(part), "missing {part:?} in:\n{md}");
        }
        let empty = Stats::collect(Scope::Task("task-003".into()), &[], None).to_markdown();
        assert!(empty.contains("Nothing was sent back."), "{empty}");
        assert!(
            empty.contains("No role has finished a step yet."),
            "{empty}"
        );
        assert!(empty.contains("No skills configured or used."), "{empty}");
        assert!(empty.contains("No agent reported its usage."), "{empty}");
    }

    #[test]
    fn saves_numbered_folders_from_the_real_history() {
        let project = tempfile::tempdir().unwrap();
        let harness = project.path();
        let runs = harness.join("runs");
        fs::create_dir(&runs).unwrap();
        let (store, mut state) = TaskStore::create(&runs, "task-001", "x", 5).unwrap();
        let design = Handoff {
            task_id: "task-001".into(),
            ..handoff(
                Role::Architect,
                Verdict::Approved,
                NextStep::To(Role::Human),
            )
        };
        let step = store.record(&mut state, &design, "").unwrap();
        let claude_result = "{\"type\":\"result\",\"total_cost_usd\":1.5,\
                             \"usage\":{\"input_tokens\":7,\"output_tokens\":3}}\n";
        fs::write(step.join("agent.log"), claude_result).unwrap();
        store.save_failure_log(1, Role::Architect, "boom").unwrap();

        let tasks = TaskHistory::load_all(&runs).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].handoffs, [design]);
        assert_eq!(tasks[0].failures, [(1, Role::Architect)]);
        let architect = Usage {
            input_tokens: 7,
            output_tokens: 3,
            cost_usd: Some(1.5),
        };
        assert_eq!(
            tasks[0].usage,
            BTreeMap::from([(Role::Architect, architect)])
        );

        let task_stats = Stats::collect(Scope::All, &tasks, None);
        let first = task_stats.save(harness).unwrap();
        assert!(first.ends_with("retros/001"));
        fs::create_dir(harness.join("retros/notes")).unwrap();
        let second = task_stats.save(harness).unwrap();
        assert!(second.ends_with("retros/002"));

        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(first.join("stats.json")).unwrap()).unwrap();
        assert_eq!(json["scope"], "all");
        assert_eq!(json["roles"]["architect"]["failed_attempts"], 1);
        assert_eq!(json["usage"]["architect"]["cost_usd"], 1.5);
        assert!(task_stats
            .to_markdown()
            .contains("| architect | 7 | 3 | $1.50 |\n| all | 7 | 3 | $1.50 |"));
        assert_eq!(
            fs::read_to_string(first.join("stats.md")).unwrap(),
            task_stats.to_markdown()
        );
    }
}
