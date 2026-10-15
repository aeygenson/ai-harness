//! The observer's tools: their descriptions for `tools/list` and the call of one.
//!
//! Every tool only reads, and says so (`readOnlyHint`), so an agent may call
//! them without asking. There is on purpose no tool that approves, sends work
//! back, adds rounds or starts a task: those are decisions made in the TUI.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use super::read;

/// How long `wait_for_change` waits when the agent does not say.
const DEFAULT_WAIT_SECONDS: u64 = 60;

/// The longest `wait_for_change` may wait: agents give up on a tool call after
/// a few minutes, and the agent can simply call it again.
const MAX_WAIT_SECONDS: u64 = 600;

/// Why a tool call got no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CallError {
    /// The agent asked for a tool the server does not have.
    UnknownTool(String),
    /// An argument is missing or has the wrong type.
    BadArguments(String),
    /// The tool ran but could not read what it needed (a missing task, a broken file).
    Failed(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::UnknownTool(name) => write!(f, "there is no tool {name:?}"),
            CallError::BadArguments(why) | CallError::Failed(why) => f.write_str(why),
        }
    }
}

/// Where the tools read from.
#[derive(Debug, Clone)]
pub(super) struct Folders<'a> {
    /// The project's `.harness` folder (with `harness.toml`).
    pub(super) harness_dir: &'a Path,
    /// The task folders, `.harness/runs`.
    pub(super) runs: &'a Path,
}

/// The tool list for `tools/list`.
pub(super) fn definitions() -> Value {
    let task = json!({ "type": "string", "description": "The task id, for example task-001." });
    json!([
        tool(
            "list_tasks",
            "Every task of the project: id, title, stage (who works or why it waits), round and round limit.",
            &json!({}),
            &[],
        ),
        tool(
            "task_status",
            "One task in detail: description, stage, every step so far (numbered) and failed attempts.",
            &json!({ "task": task }),
            &["task"],
        ),
        tool(
            "read_step",
            "One step of a task in full: the role's handoff, its notes and the end of its agent.log.",
            &json!({
                "task": task,
                "step": { "type": "integer", "minimum": 1, "description": "The step number from task_status." },
            }),
            &["task", "step"],
        ),
        tool(
            "retro_stats",
            "Retrospective statistics (rounds, returns, repeated issues, skills, tokens) of one task or of all tasks, as Markdown.",
            &json!({ "task": { "type": "string", "description": "A task id; leave out for all tasks." } }),
            &[],
        ),
        tool(
            "read_settings",
            "The project's harness.toml: roles, agents, models, skills and MCP servers. It holds no secret values.",
            &json!({}),
            &[],
        ),
        tool(
            "wait_for_change",
            "Waits until the task's state changes (a role finished, it waits for a decision, it is done) or the time runs out. Returns the new stage and whether it changed.",
            &json!({
                "task": task,
                "seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_WAIT_SECONDS,
                    "description": format!("How long to wait; {DEFAULT_WAIT_SECONDS} if left out."),
                },
            }),
            &["task"],
        ),
    ])
}

/// Runs the tool `name` with `arguments` and returns its text.
pub(super) fn call(
    folders: &Folders<'_>,
    poll: Duration,
    name: &str,
    arguments: &Value,
) -> Result<String, CallError> {
    let runs = folders.runs;
    let value = match name {
        "list_tasks" => read::list_tasks(runs),
        "task_status" => read::task_status(runs, text_argument(arguments, "task")?),
        "read_step" => {
            let step = number_argument(arguments, "step")?.unwrap_or(0);
            let step = usize::try_from(step)
                .map_err(|_too_big| CallError::BadArguments(format!("step {step} is too big")))?;
            read::read_step(runs, text_argument(arguments, "task")?, step)
        }
        "retro_stats" => {
            let task = optional_text_argument(arguments, "task")?;
            return read::retro_stats(folders.harness_dir, runs, task).map_err(CallError::Failed);
        }
        "read_settings" => {
            return read::read_settings(folders.harness_dir).map_err(CallError::Failed);
        }
        "wait_for_change" => {
            let seconds = number_argument(arguments, "seconds")?
                .unwrap_or(DEFAULT_WAIT_SECONDS)
                .clamp(1, MAX_WAIT_SECONDS);
            let task = text_argument(arguments, "task")?;
            read::wait_for_change(runs, task, Duration::from_secs(seconds), poll)
        }
        _ => return Err(CallError::UnknownTool(name.to_string())),
    };
    let value = value.map_err(CallError::Failed)?;
    serde_json::to_string_pretty(&value).map_err(|e| CallError::Failed(e.to_string()))
}

/// One entry of the tool list. All tools only read.
fn tool(name: &str, description: &str, properties: &Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
        "annotations": { "readOnlyHint": true },
    })
}

/// A text argument the tool cannot work without.
fn text_argument<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, CallError> {
    optional_text_argument(arguments, name)?
        .ok_or_else(|| CallError::BadArguments(format!("the argument {name:?} is missing")))
}

/// A text argument that may be left out.
fn optional_text_argument<'a>(
    arguments: &'a Value,
    name: &str,
) -> Result<Option<&'a str>, CallError> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(CallError::BadArguments(format!("{name:?} must be text"))),
    }
}

/// A whole-number argument that may be left out.
fn number_argument(arguments: &Value, name: &str) -> Result<Option<u64>, CallError> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| {
            CallError::BadArguments(format!("{name:?} must be a whole number of at least 0"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_only_reads() {
        let tools = definitions();
        let tools = tools.as_array().unwrap();
        assert_eq!(tools.len(), 6);
        assert!(tools
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true));
    }

    #[test]
    fn arguments_of_the_wrong_kind_are_refused() {
        let arguments = json!({ "task": 7, "step": "two" });
        assert!(matches!(
            text_argument(&arguments, "task"),
            Err(CallError::BadArguments(_))
        ));
        assert!(matches!(
            number_argument(&arguments, "step"),
            Err(CallError::BadArguments(_))
        ));
        assert!(matches!(
            text_argument(&json!({}), "task"),
            Err(CallError::BadArguments(_))
        ));
        assert_eq!(number_argument(&json!({}), "step"), Ok(None));
    }
}
