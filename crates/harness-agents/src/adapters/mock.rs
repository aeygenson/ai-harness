//! A pretend agent that follows a script. It lets us test the whole loop
//! without subscriptions, internet or waiting.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use harness_core::task::agent::{AgentOutcome, AgentRunner, RoleJob, RunEnd};
use harness_core::task::handoff::{Handoff, Issue, NextStep, Role, Severity, Verdict};

/// What the mock does the next time a given role runs.
#[derive(Debug, Clone)]
pub enum MockStep {
    /// Change project files, then write a correct `notes.md` and `handoff.json`.
    Finish {
        /// The verdict written into `handoff.json`.
        verdict: Verdict,
        /// The role (or the end) the handoff asks to go to next.
        next: NextStep,
        /// The handoff's summary; with `Rejected` it is also the text of the one issue.
        summary: String,
        /// `(path in the project, contents)` pairs.
        files: Vec<(String, String)>,
    },
    /// Exit without writing anything, like an agent that crashed quietly.
    WriteNothing,
    /// Change project files but write no handoff: the work is half done.
    WriteFilesOnly(Vec<(String, String)>),
    /// Make a git commit itself, which agents must never do.
    GitCommit,
    /// Add a setting to `.git/config`, which agents must never change.
    ChangeGitConfig,
    /// Write a `handoff.json` that is not valid.
    WriteGarbage,
    /// Pretend the subscription limit was hit.
    UsageLimit,
    /// Write these files into the output folder only, like the retrospective.
    WriteOutput(Vec<(String, String)>),
}

impl MockStep {
    /// A step that writes a valid handoff with `verdict` and `next`, changing no project files.
    pub fn finish(verdict: Verdict, next: NextStep) -> Self {
        MockStep::Finish {
            verdict,
            next,
            summary: format!("{verdict:?}"),
            files: vec![],
        }
    }

    /// Like `finish`, but first writes `files` into the project.
    pub fn finish_writing(verdict: Verdict, next: NextStep, files: &[(&str, &str)]) -> Self {
        MockStep::Finish {
            verdict,
            next,
            summary: format!("{verdict:?}"),
            files: owned(files),
        }
    }
}

/// Turns `&[("a.rs", "text")]` into owned strings.
fn owned(files: &[(&str, &str)]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect()
}

/// A scripted agent. Each role has its own queue of steps.
///
/// `Mutex` lets `run(&self)` change the queues: several tasks could share one
/// agent, and Rust makes us say how shared data is protected.
#[derive(Debug, Default)]
pub struct MockAgent {
    script: Mutex<HashMap<Role, VecDeque<MockStep>>>,
    calls: Mutex<Vec<Role>>,
}

impl MockAgent {
    /// A mock with no steps yet; add them with `then`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a step to `role`'s queue (builder style: `MockAgent::new().then(..).then(..)`).
    ///
    /// # Panics
    ///
    /// Only if another thread panicked while it held the mock's lock (a broken test).
    pub fn then(self, role: Role, step: MockStep) -> Self {
        self.script
            .lock()
            .unwrap()
            .entry(role)
            .or_default()
            .push_back(step);
        self
    }

    /// Which roles were run, in order. Useful in tests.
    ///
    /// # Panics
    ///
    /// Only if another thread panicked while it held the mock's lock (a broken test).
    pub fn calls(&self) -> Vec<Role> {
        self.calls.lock().unwrap().clone()
    }

    fn next_step(&self, role: Role) -> MockStep {
        self.script
            .lock()
            .unwrap()
            .get_mut(&role)
            .and_then(VecDeque::pop_front)
            .unwrap_or(MockStep::WriteNothing)
    }
}

impl AgentRunner for MockAgent {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        self.calls.lock().unwrap().push(job.role);
        let log = format!("mock {:?}, round {}", job.role, job.round);

        match self.next_step(job.role) {
            MockStep::UsageLimit => AgentOutcome {
                end: RunEnd::UsageLimit,
                log,
            },
            MockStep::WriteNothing => AgentOutcome {
                end: RunEnd::Succeeded,
                log,
            },
            MockStep::WriteGarbage => {
                let written = fs::write(job.output_dir.join("handoff.json"), "{ not json");
                AgentOutcome {
                    end: done_if(written.is_ok()),
                    log,
                }
            }
            MockStep::WriteOutput(files) => AgentOutcome {
                end: done_if(write_files(&job.output_dir, &files).is_ok()),
                log,
            },
            MockStep::WriteFilesOnly(files) => AgentOutcome {
                end: done_if(write_files(&job.project_dir, &files).is_ok()),
                log,
            },
            MockStep::ChangeGitConfig => {
                let changed = Command::new("git")
                    .current_dir(&job.project_dir)
                    .args(["config", "core.fsmonitor", "agent-watcher"])
                    .status();
                AgentOutcome {
                    end: done_if(changed.is_ok_and(|status| status.success())),
                    log,
                }
            }
            MockStep::GitCommit => {
                let committed = Command::new("git")
                    .current_dir(&job.project_dir)
                    .args(["-c", "user.name=agent", "-c", "user.email=agent@localhost"])
                    .args(["commit", "-q", "--allow-empty", "-m", "sneaky"])
                    .status();
                AgentOutcome {
                    end: done_if(committed.is_ok_and(|status| status.success())),
                    log,
                }
            }
            MockStep::Finish {
                verdict,
                next,
                summary,
                files,
            } => {
                let written = write_files(&job.project_dir, &files)
                    .and_then(|()| write_result(job, verdict, next, &summary));
                AgentOutcome {
                    end: done_if(written.is_ok()),
                    log,
                }
            }
        }
    }
}

/// `Succeeded` when the mock's step worked; a failed step reads like a crash.
fn done_if(worked: bool) -> RunEnd {
    if worked {
        RunEnd::Succeeded
    } else {
        RunEnd::Failed("the mock agent's step did not work".into())
    }
}

fn write_files(project_dir: &Path, files: &[(String, String)]) -> std::io::Result<()> {
    for (path, text) in files {
        let path = project_dir.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)?;
    }
    Ok(())
}

fn write_result(
    job: &RoleJob,
    verdict: Verdict,
    next: NextStep,
    summary: &str,
) -> std::io::Result<()> {
    let issues = if verdict == Verdict::Rejected {
        vec![Issue {
            severity: Severity::High,
            location: None,
            description: summary.to_string(),
        }]
    } else {
        vec![]
    };
    let handoff = Handoff {
        schema_version: 1,
        task_id: job.task_id.clone(),
        round: job.round,
        role: job.role,
        verdict,
        next_role: next,
        summary: summary.to_string(),
        skills_used: vec![],
        files: vec![],
        issues,
    };
    let json = serde_json::to_string_pretty(&handoff)?;
    fs::write(job.output_dir.join("handoff.json"), json)?;
    fs::write(
        job.output_dir.join("notes.md"),
        format!("{:?}: {summary}\n", job.role),
    )
}
