//! A pretend agent that follows a script. It lets us test the whole loop
//! without subscriptions, internet or waiting.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::future::Future;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use harness_core::task::agent::{AgentOutcome, AgentRunner, RoleJob, RunEnd};
use harness_core::task::handoff::{Handoff, Issue, NextStep, Role, Severity, Verdict};
use harness_core::task::stop_request::StopWhen;

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
    /// Check out a new branch at the same commit, then finish with an
    /// approval for the user, as if the work were done.
    SwitchBranch,
    /// Make a branch without checking it out, then finish with an approval
    /// for the user: a branch the harness might check out later.
    MakeBranch,
    /// Add a setting to `.git/config`, which agents must never change.
    ChangeGitConfig,
    /// Write a `handoff.json` that is not valid.
    WriteGarbage,
    /// Pretend the subscription limit was hit.
    UsageLimit,
    /// Change project files, then stop as if the time limit was reached.
    TimeOut(Vec<(String, String)>),
    /// Change project files, then stop as if the subscription limit was hit.
    UsageLimitAfterWriting(Vec<(String, String)>),
    /// Write these files into the output folder only, like the retrospective.
    WriteOutput(Vec<(String, String)>),
    /// Change project files, then stop as if the user pressed «Stop now».
    StopNow(Vec<(String, String)>),
    /// Change project files, then keep working until the user presses a stop
    /// button (`RoleJob::stop`): «Stop now» stops it as a real agent is
    /// stopped; «Stop after this step» lets it finish with an approval that
    /// sends the work to `next`. Gives up after `MOCK_WORK_LIMIT`.
    WorkUntilStopped {
        /// `(path in the project, contents)` pairs written at the start.
        files: Vec<(String, String)>,
        /// Where the approval sends the work when it finishes; `None`: it
        /// does not finish, only «Stop now» ends it.
        next: Option<NextStep>,
    },
}

/// How long [`MockStep::WorkUntilStopped`] waits for a stop button at most,
/// so a broken test fails instead of hanging.
const MOCK_WORK_LIMIT: Duration = Duration::from_secs(30);

/// How often [`MockStep::WorkUntilStopped`] looks at the stop buttons.
const MOCK_STOP_PAUSE: Duration = Duration::from_millis(10);

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
    prompts: Mutex<Vec<String>>,
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

    /// The prompt of every run, in order. Useful in tests.
    ///
    /// # Panics
    ///
    /// Only if another thread panicked while it held the mock's lock (a broken test).
    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    fn next_step(&self, role: Role) -> MockStep {
        self.script
            .lock()
            .unwrap()
            .get_mut(&role)
            .and_then(VecDeque::pop_front)
            .unwrap_or(MockStep::WriteNothing)
    }

    /// Does `step`, a step that does not wait, and reports how it ended.
    fn outcome(job: &RoleJob, step: MockStep, log: String) -> AgentOutcome {
        let dir = &job.project_dir;
        // Most steps only decide how the run ended; the steps that stop
        // halfway build their whole outcome and `return` it.
        let end = match step {
            MockStep::UsageLimit => RunEnd::UsageLimit,
            MockStep::TimeOut(files) => {
                let end = RunEnd::TimedOut("timed out after 1 s; the agent was stopped".into());
                return stop_after_writing(job, &files, end, log);
            }
            MockStep::UsageLimitAfterWriting(files) => {
                return stop_after_writing(job, &files, RunEnd::UsageLimit, log);
            }
            MockStep::StopNow(files) => {
                return stop_after_writing(job, &files, RunEnd::StoppedByUser, log);
            }
            // `run` handles it: it waits.
            MockStep::WorkUntilStopped { .. } => {
                RunEnd::Failed("the mock agent's step did not work".into())
            }
            MockStep::WriteNothing => RunEnd::Succeeded,
            MockStep::WriteGarbage => {
                done_if(fs::write(job.output_dir.join("handoff.json"), "{ not json").is_ok())
            }
            MockStep::WriteOutput(files) => done_if(write_files(&job.output_dir, &files).is_ok()),
            MockStep::WriteFilesOnly(files) => done_if(write_files(dir, &files).is_ok()),
            MockStep::ChangeGitConfig => {
                done_if(git_ok(dir, &["config", "core.fsmonitor", "agent-watcher"]))
            }
            MockStep::GitCommit => done_if(git_ok(
                dir,
                &[
                    "-c",
                    "user.name=agent",
                    "-c",
                    "user.email=agent@localhost",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "sneaky",
                ],
            )),
            // `step @` names the matched value so the arm can look at it.
            step @ (MockStep::SwitchBranch | MockStep::MakeBranch) => {
                let args: &[&str] = if matches!(step, MockStep::SwitchBranch) {
                    &["switch", "-q", "-c", "agent-branch"]
                } else {
                    &["branch", "harness/task-009"]
                };
                let written = git_ok(dir, args)
                    && write_result(
                        job,
                        Verdict::Approved,
                        NextStep::To(Role::Human),
                        "switched",
                    )
                    .is_ok();
                done_if(written)
            }
            MockStep::Finish {
                verdict,
                next,
                summary,
                files,
            } => done_if(
                write_files(dir, &files)
                    .and_then(|()| write_result(job, verdict, next, &summary))
                    .is_ok(),
            ),
        };
        AgentOutcome { end, log }
    }
}

impl AgentRunner for MockAgent {
    // Most steps are done at once; only `WorkUntilStopped` waits (without
    // blocking the thread, so the run can still be dropped).
    fn run(&self, job: &RoleJob) -> impl Future<Output = AgentOutcome> + Send {
        self.calls.lock().unwrap().push(job.role);
        self.prompts.lock().unwrap().push(job.prompt.clone());
        let log = format!("mock {:?}, round {}", job.role, job.round);
        let step = self.next_step(job.role);
        async move {
            match step {
                MockStep::WorkUntilStopped { files, next } => {
                    work_until_stopped(job, &files, next, log).await
                }
                step => Self::outcome(job, step, log),
            }
        }
    }
}

/// [`MockStep::WorkUntilStopped`]: writes `files`, then waits for a stop button.
async fn work_until_stopped(
    job: &RoleJob,
    files: &[(String, String)],
    next: Option<NextStep>,
    log: String,
) -> AgentOutcome {
    if write_files(&job.project_dir, files).is_err() {
        return AgentOutcome {
            end: RunEnd::Failed("the mock agent's step did not work".into()),
            log,
        };
    }
    let start = Instant::now();
    while start.elapsed() < MOCK_WORK_LIMIT {
        match (job.stop.asked(), next) {
            (Some(StopWhen::Now), _) => {
                return AgentOutcome {
                    end: RunEnd::StoppedByUser,
                    log,
                }
            }
            (Some(StopWhen::AfterStep), Some(next)) => {
                let written = write_result(job, Verdict::Approved, next, "finished");
                return AgentOutcome {
                    end: done_if(written.is_ok()),
                    log,
                };
            }
            (Some(StopWhen::AfterStep) | None, _) => tokio::time::sleep(MOCK_STOP_PAUSE).await,
        }
    }
    AgentOutcome {
        end: RunEnd::Failed("the mock agent waited too long for a stop button".into()),
        log,
    }
}

/// Writes `files` into the project, then ends with `end` (a limit); a failed
/// write reads like a crash.
fn stop_after_writing(
    job: &RoleJob,
    files: &[(String, String)],
    end: RunEnd,
    log: String,
) -> AgentOutcome {
    let end = if write_files(&job.project_dir, files).is_ok() {
        end
    } else {
        RunEnd::Failed("the mock agent's step did not work".into())
    };
    AgentOutcome { end, log }
}

/// Runs `git` with `args` in `project_dir`, as an agent would; `true` when it worked.
fn git_ok(project_dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(project_dir)
        .args(args)
        .status()
        .is_ok_and(|status| status.success())
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
        answer_to: None,
    };
    let json = serde_json::to_string_pretty(&handoff)?;
    fs::write(job.output_dir.join("handoff.json"), json)?;
    fs::write(
        job.output_dir.join("notes.md"),
        format!("{:?}: {summary}\n", job.role),
    )
}
