//! The server's tools: their descriptions for `tools/list` and the call of one.
//!
//! The reading tools say so (`readOnlyHint`), so an agent may call them
//! without asking; the tools that act (see `actions`) do not.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use super::decisions::{self, Decide};
use super::{actions, read};

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

/// Where the tools read from and work in.
#[derive(Debug, Clone)]
pub(super) struct Folders<'a> {
    /// The project folder (a git repository).
    pub(super) root: &'a Path,
    /// The `harness` program `start_run` starts.
    pub(super) harness: &'a Path,
    /// The project's `.harness` folder (with `harness.toml`).
    pub(super) harness_dir: &'a Path,
    /// The task folders, `.harness/runs`.
    pub(super) runs: &'a Path,
}

/// The tool list for `tools/list`.
pub(super) fn definitions() -> Value {
    let task = json!({ "type": "string", "description": "The task id, for example task-001." });
    let mut tools = json!([
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
        tool(
            "read_run_log",
            "The end of what the harness printed in the last run started with start_run: why it stopped and what comes next.",
            &json!({ "task": task }),
            &["task"],
        ),
    ]);
    tools
        .as_array_mut()
        .expect("the list above is an array")
        .extend(action_definitions(&task));
    tools
}

/// The tools that change something, for [`definitions`].
fn action_definitions(task: &Value) -> Vec<Value> {
    let seen_steps = json!({
        "type": "integer",
        "minimum": 0,
        "description": "How many steps task_status showed: refused if the task has moved on since.",
    });
    vec![
        action(
            "create_task",
            "Creates a task and commits it, like `harness task new`. Refused when an unfinished task has the same text (unless allow_duplicate) or another harness works in the project.",
            &json!({
                "description": { "type": "string", "description": "What the task should do; the first line is its title." },
                "task": { "type": "string", "description": "The id, for example task-012; the next free one if left out." },
                "allow_duplicate": { "type": "boolean", "description": "Create it even if an unfinished task has the same text." },
            }),
            &["description"],
        ),
        action(
            "start_run",
            "Starts the task's roles in the background, like `harness run`, and answers within a few seconds. It runs until the task is done or needs a person; a stopped task continues, a done one is sent to GitHub again.",
            &json!({ "task": task }),
            &["task"],
        ),
        action(
            "stop_run",
            "Asks the harness running the task (here, in the TUI or a terminal) to stop once the running role has finished, or at once with now.",
            &json!({
                "task": task,
                "now": { "type": "boolean", "description": "Stop the running agent at once; its unfinished work is kept." },
            }),
            &["task"],
        ),
        action(
            "decide",
            "The user's decision on a task waiting for them: approve the architect's design (or send it back with to=architect), answer a role's question, send work back, or give a task at its round limit 5 more rounds. Saved like `harness approve`, marked as sent through MCP; then call start_run. Without `to`: the developer after a design, the role a question suggests (else the one that asked), the stopped role.",
            &json!({
                "task": task,
                "seen_steps": seen_steps,
                "to": { "type": "string", "description": "The role that continues: architect, developer, tester or security." },
                "notes": { "type": "string", "description": "What that role should know or do; the first line is the summary." },
            }),
            &["task", "seen_steps", "notes"],
        ),
        action(
            "finish_task",
            "Accepts the result and finishes the task (after the architect's final review handed it to the user, at the round limit, or when stopped). Only with the user's own words of acceptance, as they wrote them; they are saved in the history.",
            &json!({
                "task": task,
                "seen_steps": seen_steps,
                "user_words": { "type": "string", "description": "The user's own message accepting the result, word for word." },
            }),
            &["task", "seen_steps", "user_words"],
        ),
    ]
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
        "create_task" => {
            let description = text_argument(arguments, "description")?;
            let task = optional_text_argument(arguments, "task")?;
            let duplicate = bool_argument(arguments, "allow_duplicate")?;
            return actions::create_task(folders.root, task, description, duplicate)
                .map_err(CallError::Failed);
        }
        "start_run" => {
            let task = text_argument(arguments, "task")?;
            return actions::start_run(folders.root, folders.harness, task)
                .map_err(CallError::Failed);
        }
        "stop_run" => {
            let now = bool_argument(arguments, "now")?;
            return actions::stop_run(folders.root, text_argument(arguments, "task")?, now)
                .map_err(CallError::Failed);
        }
        "decide" => {
            let ask = Decide {
                task: text_argument(arguments, "task")?,
                seen_steps: count_argument(arguments, "seen_steps")?,
                to: optional_text_argument(arguments, "to")?,
                notes: text_argument(arguments, "notes")?,
            };
            return decisions::decide(folders.root, ask).map_err(CallError::Failed);
        }
        "finish_task" => {
            let task = text_argument(arguments, "task")?;
            let seen = count_argument(arguments, "seen_steps")?;
            let words = text_argument(arguments, "user_words")?;
            return decisions::finish_task(folders.root, task, seen, words)
                .map_err(CallError::Failed);
        }
        "read_run_log" => {
            return actions::read_run_log(folders.root, text_argument(arguments, "task")?)
                .map_err(CallError::Failed);
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

/// One entry of the tool list, for a tool that only reads.
fn tool(name: &str, description: &str, properties: &Value, required: &[&str]) -> Value {
    entry(name, description, properties, required, true)
}

/// One entry of the tool list, for a tool that changes something.
fn action(name: &str, description: &str, properties: &Value, required: &[&str]) -> Value {
    entry(name, description, properties, required, false)
}

fn entry(
    name: &str,
    description: &str,
    properties: &Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
        "annotations": { "readOnlyHint": read_only },
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

/// A count the tool cannot work without, such as `seen_steps`.
fn count_argument(arguments: &Value, name: &str) -> Result<usize, CallError> {
    let count = number_argument(arguments, name)?
        .ok_or_else(|| CallError::BadArguments(format!("the argument {name:?} is missing")))?;
    usize::try_from(count)
        .map_err(|_too_big| CallError::BadArguments(format!("{name:?} is too big")))
}

/// A yes-or-no argument; left out means no.
fn bool_argument(arguments: &Value, name: &str) -> Result<bool, CallError> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(CallError::BadArguments(format!(
            "{name:?} must be true or false"
        ))),
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
    fn only_the_reading_tools_say_they_only_read() {
        let tools = definitions();
        let acting: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["annotations"]["readOnlyHint"] == false)
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            acting,
            [
                "create_task",
                "start_run",
                "stop_run",
                "decide",
                "finish_task"
            ]
        );
        assert_eq!(tools.as_array().unwrap().len(), 12);
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
        assert_eq!(bool_argument(&json!({}), "now"), Ok(false));
        assert!(matches!(
            bool_argument(&json!({ "now": "yes" }), "now"),
            Err(CallError::BadArguments(_))
        ));
    }
}
