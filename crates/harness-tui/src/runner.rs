//! Runs roles in the background, so the TUI keeps working while an agent
//! does. The same core functions as `harness task new`, `harness approve` and
//! `harness run`, in a thread with its own small tokio runtime. What the agent
//! prints comes back line by line for the live log.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use harness_agents::build::BuildError;
use harness_agents::process;
use harness_agents::Team;
use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::handoff::{NextStep, Role, Verdict};
use harness_core::orchestrator::{self, StopReason};
use harness_core::skills::Skills;
use harness_core::store::{next_task_id, TaskStore};

/// Builds the agents of the project: `build_team`, or mock agents in tests.
pub type Builder = fn(&Config, &Path) -> Result<Team, BuildError>;

/// The live log keeps this many lines.
pub const LOG_LINES: usize = 500;

/// What Lisa sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// A new task with this description; the architect starts.
    New(String),
    /// Lisa's decision on a waiting task; then the roles run, unless it is done.
    Decide {
        task: String,
        verdict: Verdict,
        next: NextStep,
        notes: String,
    },
    /// A role stopped part-way (a limit, a failure): run the task again.
    Continue(String),
}

/// The model and level Lisa chose for one role for this launch only;
/// `harness.toml` stays as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunChoice {
    pub role: Role,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// How the work ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub task: String,
    /// Why the roles stopped; `None` when nothing had to run.
    pub stop: Option<StopReason>,
}

#[derive(Debug)]
enum Event {
    /// The task being worked on; a new task gets its id here.
    Started(String),
    Finished(Result<Outcome, String>),
}

/// Work in the background.
#[derive(Debug)]
pub struct Running {
    events: Receiver<Event>,
    log: Receiver<String>,
    /// The task, once known.
    pub task: Option<String>,
}

impl Running {
    pub fn start(
        root: &Path,
        request: Request,
        choice: Option<RunChoice>,
        builder: Builder,
    ) -> Self {
        let (events, events_rx) = channel();
        let (log, log_rx) = channel();
        let root = root.to_path_buf();
        thread::spawn(move || {
            process::set_live_log(Some(log));
            let result = work(&root, request, choice, builder, &events);
            process::set_live_log(None);
            let _ = events.send(Event::Finished(result));
        });
        Self {
            events: events_rx,
            log: log_rx,
            task: None,
        }
    }

    /// Takes what arrived: the agent's lines go to `log` (the latest
    /// [`LOG_LINES`] stay). `Some` once the work is over.
    pub fn poll(&mut self, log: &mut VecDeque<String>) -> Option<Result<Outcome, String>> {
        for line in self.log.try_iter() {
            if let Some(line) = readable(&line) {
                push_line(log, line);
            }
        }
        for event in self.events.try_iter() {
            match event {
                Event::Started(task) => self.task = Some(task),
                Event::Finished(result) => return Some(result),
            }
        }
        None
    }
}

/// Adds a line to the live log, dropping the oldest beyond [`LOG_LINES`].
pub fn push_line(log: &mut VecDeque<String>, line: String) {
    if log.len() >= LOG_LINES {
        log.pop_front();
    }
    log.push_back(line);
}

fn work(
    root: &Path,
    request: Request,
    choice: Option<RunChoice>,
    builder: Builder,
    events: &Sender<Event>,
) -> Result<Outcome, String> {
    let text = |e: &dyn std::fmt::Display| e.to_string();
    let repo = Repo::open(root).map_err(|e| text(&e))?;
    let harness_dir = root.join(HARNESS_DIR);
    let mut config = Config::load(&harness_dir).map_err(|e| text(&e))?;
    if let Some(choice) = choice {
        if let Some(settings) = config.roles.get_mut(&choice.role) {
            settings.model = choice.model;
            settings.effort = choice.effort;
        }
    }
    let runs = repo.runs_dir();
    // Everything the agents need is checked before anything is saved: a
    // missing login leaves the task as it was.
    let needs_run = !matches!(
        request,
        Request::Decide {
            next: NextStep::Done,
            ..
        }
    );
    let agents = if needs_run {
        let team = builder(&config, root).map_err(|e| text(&e))?;
        let skills = Skills::load(&harness_dir, &config).map_err(|e| text(&e))?;
        Some((team, skills))
    } else {
        None
    };

    let (store, mut state) = match request {
        Request::New(description) => {
            let id = next_task_id(&runs).map_err(|e| text(&e))?;
            orchestrator::create_task(&repo, &id, &description, config.max_rounds)
                .map_err(|e| text(&e))?
        }
        Request::Decide {
            task,
            verdict,
            next,
            notes,
        } => {
            let (store, mut state) = TaskStore::open(&runs, &task).map_err(|e| text(&e))?;
            orchestrator::record_human_decision(&repo, &store, &mut state, verdict, next, &notes)
                .map_err(|e| text(&e))?;
            (store, state)
        }
        Request::Continue(task) => TaskStore::open(&runs, &task).map_err(|e| text(&e))?,
    };
    let task = state.task_id.clone();
    let _ = events.send(Event::Started(task.clone()));
    let Some((team, skills)) = agents else {
        return Ok(Outcome { task, stop: None });
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    let stop = runtime
        .block_on(orchestrator::run_with_skills(
            &repo, &store, &mut state, &team, &skills,
        ))
        .map_err(|e| text(&e))?;
    Ok(Outcome {
        task,
        stop: Some(stop),
    })
}

/// A line of the agent's output as the live log shows it. The agents print
/// JSON events; the texts in them are enough to follow the work. Empty events
/// (`None`) are left out.
pub fn readable(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Some(shorten(line));
    };
    let kind = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let mut texts = Vec::new();
    collect_texts(&value, &mut texts);
    if texts.is_empty() {
        return None;
    }
    Some(shorten(&format!("{kind:<14} {}", texts.join(" · "))))
}

/// Keys whose string values say what the agent does.
const TEXT_KEYS: [&str; 7] = [
    "text",
    "command",
    "name",
    "tool_name",
    "result",
    "content",
    "message",
];

fn collect_texts(value: &serde_json::Value, out: &mut Vec<String>) {
    if out.len() >= 3 {
        return;
    }
    match value {
        serde_json::Value::Object(map) => {
            // The object's own texts first (a tool's name), then what is
            // inside it (the tool's command).
            for (key, value) in map {
                if let serde_json::Value::String(text) = value {
                    if TEXT_KEYS.contains(&key.as_str()) && !text.trim().is_empty() {
                        out.push(text.split_whitespace().collect::<Vec<_>>().join(" "));
                    }
                }
            }
            for value in map.values() {
                if value.is_object() || value.is_array() {
                    collect_texts(value, out);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_texts(item, out);
            }
        }
        _ => {}
    }
}

/// At most 300 characters: the panel cuts at its width anyway.
fn shorten(text: &str) -> String {
    let mut short: String = text.chars().take(300).collect();
    if short.len() < text.len() {
        short.push('…');
    }
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_events_become_short_readable_lines() {
        let claude = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        assert_eq!(
            readable(claude).unwrap(),
            "assistant      Bash · cargo test"
        );
        let codex = r#"{"type":"item.completed","item":{"type":"agent_message","text":"All\n tests pass"}}"#;
        assert_eq!(readable(codex).unwrap(), "item.completed All tests pass");
        // Events without text are left out; plain text stays.
        assert_eq!(readable(r#"{"type":"turn.started"}"#), None);
        assert_eq!(readable("  "), None);
        assert_eq!(readable("warning: slow").unwrap(), "warning: slow");
        let long = "x".repeat(400);
        assert_eq!(readable(&long).unwrap().chars().count(), 301);
    }

    #[test]
    fn the_log_keeps_the_latest_lines() {
        let mut log = VecDeque::new();
        for i in 0..LOG_LINES + 5 {
            push_line(&mut log, i.to_string());
        }
        assert_eq!(log.len(), LOG_LINES);
        assert_eq!(log.front().unwrap(), "5");
    }
}
