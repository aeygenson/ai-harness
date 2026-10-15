//! What the observer's tools read: tasks, their steps, statistics and settings.
//!
//! Nothing here writes a file. Every text an agent wrote goes through
//! `text::safe` before it leaves (handoffs are already cleaned when they are read).

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use harness_core::config::{Config, CONFIG_FILE};
use harness_core::retro::{stage_text, Scope, Stats, TaskHistory};
use harness_core::task::store::{self, Step, TaskStore};
use harness_core::task::TaskState;
use harness_core::text::{safe, safe_line};
use serde_json::{json, Value};

/// Only the end of a long `agent.log` is sent: a role's log can be megabytes,
/// and the end (the last commands and the error, if any) is what explains a step.
pub(super) const LOG_TAIL_CHARS: usize = 20_000;

/// A task's title in the list is the first line of `task.md`, cut to this many characters.
const TITLE_CHARS: usize = 120;

/// An error message from the store is cut to this many characters.
const ERROR_CHARS: usize = 300;

/// Every task of the project with its stage and round; a task that cannot be
/// read is listed with the reason.
pub(super) fn list_tasks(runs: &Path) -> Result<Value, String> {
    let ids = store::task_ids(runs).map_err(|e| e.to_string())?;
    let tasks: Vec<Value> = ids
        .iter()
        .map(|id| match TaskStore::open(runs, id) {
            Ok((store, state)) => {
                let mut row = state_json(&state);
                let description = store.description().unwrap_or_default();
                row["title"] = json!(safe_line(
                    description.lines().next().unwrap_or_default(),
                    TITLE_CHARS
                ));
                row
            }
            Err(error) => json!({ "id": id, "error": safe_line(&error.to_string(), ERROR_CHARS) }),
        })
        .collect();
    Ok(json!(tasks))
}

/// One task in detail: its state, description, numbered steps and failed attempts.
pub(super) fn task_status(runs: &Path, task: &str) -> Result<Value, String> {
    let (store, state) = TaskStore::open(runs, task).map_err(|e| e.to_string())?;
    let mut status = state_json(&state);
    status["description"] = json!(safe(&store.description().unwrap_or_default()));
    let steps = store.steps().map_err(|e| e.to_string())?;
    // Steps are numbered from 1, the number `read_step` takes.
    status["steps"] = steps
        .iter()
        .enumerate()
        .map(|(index, step)| step_row(index + 1, step))
        .collect();
    let failures = store.failures().map_err(|e| e.to_string())?;
    status["failed_attempts"] = failures
        .iter()
        .map(|(round, role)| json!({ "round": round, "role": role.as_str() }))
        .collect();
    Ok(status)
}

/// One step in full: the handoff, the notes and the end of the agent's log.
pub(super) fn read_step(runs: &Path, task: &str, number: usize) -> Result<Value, String> {
    let (store, _) = TaskStore::open(runs, task).map_err(|e| e.to_string())?;
    let steps = store.steps().map_err(|e| e.to_string())?;
    // `checked_sub` gives `None` for step 0 instead of wrapping around.
    let Some(step) = number.checked_sub(1).and_then(|index| steps.get(index)) else {
        return Err(format!(
            "{task} has {} steps; there is no step {number}",
            steps.len()
        ));
    };
    let log = step.agent_log().unwrap_or_default();
    let (log_tail, log_cut) = tail(&safe(&log), LOG_TAIL_CHARS);
    Ok(json!({
        "step": number,
        "folder": step_folder(step),
        "handoff": step.handoff,
        "notes": safe(&step.notes),
        "agent_log": log_tail,
        "agent_log_cut": log_cut,
    }))
}

/// The retrospective statistics of one task, or of all of them, as Markdown.
/// Counted from the saved history only: no file is written and no agent runs.
pub(super) fn retro_stats(
    harness_dir: &Path,
    runs: &Path,
    task: Option<&str>,
) -> Result<String, String> {
    let (scope, histories) = match task {
        Some(id) => {
            let history = TaskHistory::load(runs, id).map_err(|e| e.to_string())?;
            (Scope::Task(id.to_string()), vec![history])
        }
        None => (
            Scope::All,
            TaskHistory::load_all(runs).map_err(|e| e.to_string())?,
        ),
    };
    // Without a readable harness.toml the skills are just not compared, as in `harness retro`.
    let config = Config::load(harness_dir).ok();
    Ok(Stats::collect(scope, &histories, config.as_ref()).to_markdown())
}

/// The project's `harness.toml` as text. Secrets are never in it: it names
/// them (`secret:<name>`), their values live in `~/.harness/credentials/`.
pub(super) fn read_settings(harness_dir: &Path) -> Result<String, String> {
    let path = harness_dir.join(CONFIG_FILE);
    fs::read_to_string(&path)
        .map(|text| safe(&text))
        .map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Waits until the task's `state.json` changes or `limit` passes, looking every `poll`.
pub(super) fn wait_for_change(
    runs: &Path,
    task: &str,
    limit: Duration,
    poll: Duration,
) -> Result<Value, String> {
    let (_, start) = TaskStore::open(runs, task).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + limit;
    loop {
        // A failed read is tried again: the harness may be writing the file right now.
        if let Ok((_, now)) = TaskStore::open(runs, task) {
            if now != start {
                let mut answer = state_json(&now);
                answer["changed"] = json!(true);
                return Ok(answer);
            }
        }
        if Instant::now() >= deadline {
            let mut answer = state_json(&start);
            answer["changed"] = json!(false);
            return Ok(answer);
        }
        thread::sleep(poll);
    }
}

/// The parts of `state.json` every answer shows.
fn state_json(state: &TaskState) -> Value {
    json!({
        "id": state.task_id,
        "stage": stage_text(state.stage),
        "round": state.round,
        "max_rounds": state.max_rounds,
        "pull_request": state.pull_request.as_ref().and_then(|p| p.page()),
    })
}

/// One line of the step list in `task_status`.
fn step_row(number: usize, step: &Step) -> Value {
    let handoff = &step.handoff;
    json!({
        "step": number,
        "folder": step_folder(step),
        "round": handoff.round,
        "role": handoff.role.as_str(),
        "verdict": handoff.verdict,
        "next": handoff.next_role,
        "summary": handoff.summary,
        "issues": handoff.issues.len(),
    })
}

/// The step's place in the task folder, for example `round-02/03-tester`.
fn step_folder(step: &Step) -> String {
    let mut parts = step.dir.iter().rev().take(2).collect::<Vec<_>>();
    parts.reverse();
    parts
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The last `max_chars` characters of `text`, and whether anything was cut.
fn tail(text: &str, max_chars: usize) -> (String, bool) {
    let count = text.chars().count();
    if count <= max_chars {
        return (text.to_string(), false);
    }
    // Counting characters, not bytes, so a letter is never cut in half.
    (text.chars().skip(count - max_chars).collect(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_short_text_and_cuts_long_text_by_characters() {
        assert_eq!(tail("abc", 5), ("abc".to_string(), false));
        assert_eq!(tail("привет", 3), ("вет".to_string(), true));
    }
}
