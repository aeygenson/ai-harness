//! A pretend agent that follows a script. It lets us test the whole loop
//! without subscriptions, internet or waiting.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::sync::Mutex;

use harness_core::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::handoff::{Handoff, Issue, NextStep, Role, Severity, Verdict};

/// What the mock does the next time a given role runs.
#[derive(Debug, Clone)]
pub enum MockStep {
    /// Write a correct `notes.md` and `handoff.json`.
    Finish {
        verdict: Verdict,
        next: NextStep,
        summary: String,
    },
    /// Exit without writing anything, like an agent that crashed quietly.
    WriteNothing,
    /// Write a `handoff.json` that is not valid.
    WriteGarbage,
    /// Pretend the subscription limit was hit.
    UsageLimit,
}

impl MockStep {
    pub fn finish(verdict: Verdict, next: NextStep) -> Self {
        MockStep::Finish {
            verdict,
            next,
            summary: format!("{verdict:?}"),
        }
    }
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a step to `role`'s queue (builder style: `MockAgent::new().then(..).then(..)`).
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
    pub fn calls(&self) -> Vec<Role> {
        self.calls.lock().unwrap().clone()
    }

    fn next_step(&self, role: Role) -> MockStep {
        self.script
            .lock()
            .unwrap()
            .get_mut(&role)
            .and_then(|queue| queue.pop_front())
            .unwrap_or(MockStep::WriteNothing)
    }
}

impl AgentRunner for MockAgent {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        self.calls.lock().unwrap().push(job.role);
        let log = format!("mock {:?}, round {}", job.role, job.round);

        match self.next_step(job.role) {
            MockStep::UsageLimit => AgentOutcome {
                success: false,
                usage_limit_reached: true,
                log,
            },
            MockStep::WriteNothing => AgentOutcome {
                success: true,
                usage_limit_reached: false,
                log,
            },
            MockStep::WriteGarbage => {
                let written = fs::write(job.output_dir.join("handoff.json"), "{ not json");
                AgentOutcome {
                    success: written.is_ok(),
                    usage_limit_reached: false,
                    log,
                }
            }
            MockStep::Finish {
                verdict,
                next,
                summary,
            } => {
                let written = write_result(job, verdict, next, &summary);
                AgentOutcome {
                    success: written.is_ok(),
                    usage_limit_reached: false,
                    log,
                }
            }
        }
    }
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
