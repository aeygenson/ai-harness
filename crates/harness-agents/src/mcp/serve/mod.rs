//! `harness mcp-serve`: the harness as an MCP server.
//!
//! An AI agent on the user's computer (Claude Code, Codex, Gemini, ...) starts it
//! like any MCP server and talks to it on standard input and output, one JSON
//! message per line. Through it the agent sees the project's tasks, their
//! steps, logs, statistics and settings, and drives the tasks the way the
//! user does in the TUI: it creates a task, starts and stops its roles.
//! Every agent that speaks MCP can do this, not only one that can run shell
//! commands. Design: `docs/design.md`, section 5.8.

mod actions;
mod read;
mod tools;

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use harness_core::git::HARNESS_DIR;
use serde_json::{json, Value};

use self::tools::{CallError, Folders};

/// The MCP version the server speaks when the agent asks for one it does not know.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// The MCP versions the server can answer in. It uses only tools, which are
/// the same in all of them.
const KNOWN_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", PROTOCOL_VERSION];

/// How often `wait_for_change` looks at `state.json`: often enough to notice a
/// role finishing, rarely enough to cost nothing.
const POLL: Duration = Duration::from_secs(2);

/// JSON-RPC error codes (from the JSON-RPC 2.0 specification).
const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// What the agent reads when it connects, so it knows its place.
const INSTRUCTIONS: &str = "The AI harness in this project: a conductor that leads each task \
through roles (architect, developer, tester, security), each an AI agent. Read its tasks, \
their steps (each role's handoff, notes and log), retrospective statistics and settings, and \
drive tasks for the person you work for: create_task, start_run (runs in the background; \
follow it with wait_for_change and task_status), stop_run and read_run_log. Only one harness \
works in a project at a time. Decisions that are the person's (approving a design, accepting \
a result, merging) are theirs: ask them, do not decide for them.";

/// The MCP server for one project.
#[derive(Debug, Clone)]
pub struct Observer {
    root: PathBuf,
    harness_dir: PathBuf,
    runs: PathBuf,
    /// The `harness` program `start_run` starts: this program itself.
    harness: PathBuf,
    poll: Duration,
}

impl Observer {
    /// A server for the project in `root` (the folder with `.harness/`).
    pub fn new(root: &Path) -> Self {
        let harness_dir = root.join(HARNESS_DIR);
        Self {
            root: root.to_path_buf(),
            runs: harness_dir.join("runs"),
            harness_dir,
            // Without its own path (very rare) `harness` is looked up in PATH.
            harness: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("harness")),
            poll: POLL,
        }
    }

    /// The same server starting `harness` as the harness program: a fake in tests.
    pub fn with_program(mut self, harness: &Path) -> Self {
        self.harness = harness.to_path_buf();
        self
    }

    /// Answers the messages on `input` until it ends, writing the answers to `output`.
    pub fn serve(&self, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(answer) = self.answer(&line) {
                writeln!(output, "{answer}")?;
                // The agent waits for this line, so it must not sit in a buffer.
                output.flush()?;
            }
        }
        Ok(())
    }

    /// The answer to one message; `None` for a notification, which gets none.
    fn answer(&self, line: &str) -> Option<Value> {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return Some(error(&Value::Null, PARSE_ERROR, "the message is not JSON"));
        };
        // Notifications (and answers from the agent) have no id or no method.
        let id = message.get("id")?;
        let method = message.get("method")?.as_str().unwrap_or_default();
        let params = &message["params"];
        let result = match method {
            "initialize" => Ok(initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::definitions() })),
            "tools/call" => self.call(params),
            _ => Err((METHOD_NOT_FOUND, format!("unknown method {method:?}"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, why)) => error(id, code, &why),
        })
    }

    /// `tools/call`. A tool that runs but fails answers with `isError`, as MCP
    /// asks, so the agent sees why; a wrong tool or argument is a protocol error.
    fn call(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params["name"].as_str().unwrap_or_default();
        let empty = json!({});
        let arguments = params.get("arguments").unwrap_or(&empty);
        let folders = Folders {
            root: &self.root,
            harness: &self.harness,
            harness_dir: &self.harness_dir,
            runs: &self.runs,
        };
        match tools::call(&folders, self.poll, name, arguments) {
            Ok(text) => Ok(tool_result(&text, false)),
            Err(failed @ CallError::Failed(_)) => Ok(tool_result(&failed.to_string(), true)),
            Err(other) => Err((INVALID_PARAMS, other.to_string())),
        }
    }
}

/// `harness mcp-serve`: serves the project in `root` on standard input and output.
pub fn run(root: &Path) -> io::Result<()> {
    let stdin = io::stdin();
    Observer::new(root).serve(stdin.lock(), io::stdout())
}

/// The answer to `initialize`: the version, the server's name and what it offers.
fn initialize(params: &Value) -> Value {
    let asked = params["protocolVersion"].as_str().unwrap_or_default();
    let version = if KNOWN_VERSIONS.contains(&asked) {
        asked
    } else {
        PROTOCOL_VERSION
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "harness", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

/// A tool's answer: one text block.
fn tool_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// A JSON-RPC error answer.
fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests;
