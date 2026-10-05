//! Runs roles in the background, so the TUI keeps working while an agent
//! does. The same core functions as `harness task new`, `harness approve` and
//! `harness run`, in a thread with its own small tokio runtime. What the agent
//! prints comes back line by line for the live log.

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};

use harness_agents::build::BuildError;
use harness_agents::process;
use harness_agents::Team;
use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::handoff::{NextStep, Role, Verdict};
use harness_core::orchestrator::{self, StopReason};
use harness_core::skills::Skills;
use harness_core::store::{next_task_id, TaskStore};
use harness_core::text;
use tokio::sync::oneshot;

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

/// A thread that runs agents. Dropping it (the TUI quits, the project is
/// removed) stops the agents with everything they started, then waits for the
/// thread, so no agent keeps working after the harness is gone.
#[derive(Debug)]
pub struct Background {
    /// Dropping this sender is the stop signal (see [`run_until_stopped`]).
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Background {
    /// Starts `job` on a new thread. `job` gets the stop signal and passes it
    /// to [`run_until_stopped`].
    pub fn start(job: impl FnOnce(oneshot::Receiver<()>) + Send + 'static) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let thread = thread::spawn(move || job(stop_rx));
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        // `take` moves the sender out, and it is dropped right here.
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Runs `work` on a small tokio runtime of its own until it ends or `stop`
/// fires. Stopping drops `work`, and with it the running agent, which stops
/// everything the agent started (`harness_agents::process::run`).
pub fn run_until_stopped<F: Future>(
    work: F,
    stop: oneshot::Receiver<()>,
) -> Result<F::Output, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    runtime.block_on(async {
        // `select!` waits for whichever finishes first and drops the other.
        // The stop signal finishes when its sender is dropped.
        tokio::select! {
            output = work => Ok(output),
            _ = stop => Err("stopped".to_string()),
        }
    })
}

/// What one background run is asked to do.
struct Job {
    root: PathBuf,
    request: Request,
    choice: Option<RunChoice>,
    builder: Builder,
}

/// Work in the background.
#[derive(Debug)]
pub struct Running {
    events: Receiver<Event>,
    log: Receiver<String>,
    /// The task, once known.
    pub task: Option<String>,
    /// Kept to stop the agents when this is dropped.
    _background: Background,
}

impl Running {
    /// Starts `request` on a background thread; [`Running::poll`] collects the results.
    pub fn start(
        root: &Path,
        request: Request,
        choice: Option<RunChoice>,
        builder: Builder,
    ) -> Self {
        let (events, events_rx) = channel();
        let (log, log_rx) = channel();
        let job = Job {
            root: root.to_path_buf(),
            request,
            choice,
            builder,
        };
        let background = Background::start(move |stop| {
            process::set_live_log(Some(log));
            let result = work(job, &events, stop);
            process::set_live_log(None);
            let _ = events.send(Event::Finished(result));
        });
        Self {
            events: events_rx,
            log: log_rx,
            task: None,
            _background: background,
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

fn work(job: Job, events: &Sender<Event>, stop: oneshot::Receiver<()>) -> Result<Outcome, String> {
    let Job {
        root,
        request,
        choice,
        builder,
    } = job;
    let root = root.as_path();
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
    let stop = run_until_stopped(
        orchestrator::run_with_skills(&repo, &store, &mut state, &team, &skills),
        stop,
    )?
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

/// At most 300 characters (the panel cuts at its width anyway), without
/// terminal control characters, which agents may print.
fn shorten(text: &str) -> String {
    let text = text::safe(text);
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
        // Colours and screen commands from the agent do not reach the panel.
        assert_eq!(readable("\u{1b}[31mred\u{1b}[0m").unwrap(), "red");
        let event = r#"{"type":"assistant","text":"done\u001b[2J"}"#;
        assert_eq!(readable(event).unwrap(), "assistant      done");
        let long = "x".repeat(400);
        assert_eq!(readable(&long).unwrap().chars().count(), 301);
    }

    #[test]
    fn dropping_a_background_job_stops_its_work_and_waits() {
        let (sender, results) = channel();
        // Work that would never end by itself, like an agent that hangs.
        let background = Background::start(move |stop| {
            let result = run_until_stopped(std::future::pending::<()>(), stop);
            sender.send(result).unwrap();
        });

        drop(background);

        // `drop` waited for the thread, so its answer is already there.
        assert_eq!(results.try_recv().unwrap(), Err("stopped".to_string()));
    }

    #[test]
    fn work_that_ends_by_itself_gives_its_result() {
        let (_stop, stop_rx) = oneshot::channel::<()>();
        assert_eq!(run_until_stopped(async { 42 }, stop_rx), Ok(42));
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
