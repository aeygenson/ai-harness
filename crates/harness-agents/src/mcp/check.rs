//! «Check» on the MCP tab: starts one MCP server the way an agent would and
//! asks it which tools it has (`initialize`, then `tools/list`). No model is
//! involved. The server runs in the project folder with only the whitelisted
//! environment plus its own variables, and is stopped right after.
//!
//! What the server says is shown in the TUI, so control characters are
//! removed from it; its secrets never appear in an error.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use harness_core::mcp::tools::Tool;
use harness_core::mcp::McpServer;
use harness_core::secret::Secret;
use harness_core::text::safe_line;
use serde_json::{json, Value};

use crate::process::base_command;

/// How long the server may take, all questions together. `npx` may first
/// have to download it.
pub const TIME_LIMIT: Duration = Duration::from_mins(2);

/// The MCP version the harness speaks; servers answer with theirs.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// At most this many pages of `tools/list`.
const MAX_PAGES: usize = 20;

/// Starts `server` in `project_dir` and returns its tools.
pub fn list_tools(server: &McpServer, project_dir: &Path) -> Result<Vec<Tool>, String> {
    list_tools_within(server, project_dir, TIME_LIMIT)
}

/// Like `list_tools`, but waits at most `limit` for the server instead of [`TIME_LIMIT`].
/// Starts the server as a process and stops it, with everything it started, at the end.
pub fn list_tools_within(
    server: &McpServer,
    project_dir: &Path,
    limit: Duration,
) -> Result<Vec<Tool>, String> {
    let secrets: Vec<&str> = server.env.values().map(Secret::expose).collect();
    let hide = |text: String| harness_core::secret::hide(&text, &secrets);
    let mut command = base_command(Path::new(&server.command), project_dir);
    // `npx` starts the real server as a child; both are stopped together.
    harness_platform::process::own_group(&mut command);
    command
        .args(&server.args)
        .envs(server.env.iter().map(|(k, v)| (k, v.expose())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::process::spawn(&mut command)
        .map_err(|e| format!("cannot start {}: {e}", server.command))?;
    let (stderr_tx, stderr_rx) = channel();
    if let Some(mut pipe) = child.stderr.take() {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            let _ = stderr_tx.send(text);
        });
    }
    let mut session = Session::new(&mut child, limit);
    let result = session.ask_tools();
    // Closing standard input asks it to stop; then it is stopped.
    drop(session);
    stop(&mut child);
    let stderr = stderr_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_default();
    result.map_err(|why| {
        let said = stderr
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .map(|l| format!(" ({})", safe_line(l, 200)))
            .unwrap_or_default();
        hide(format!("{}: {why}{said}", server.name))
    })
}

/// Gives the server a moment to end, then ends it and everything it started.
fn stop(child: &mut Child) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    harness_platform::process::kill_tree(child);
}

/// The questions and answers on the server's standard input and output.
struct Session {
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    deadline: Instant,
    next_id: u64,
}

impl Session {
    fn new(child: &mut Child, limit: Duration) -> Self {
        let (tx, lines) = channel();
        if let Some(stdout) = child.stdout.take() {
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        Self {
            stdin: child.stdin.take(),
            lines,
            deadline: Instant::now() + limit,
            next_id: 1,
        }
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("no standard input")?;
        writeln!(stdin, "{message}")
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("the server stopped: {error}"))
    }

    /// Sends a request and waits for the answer with its id; notifications
    /// and the server's own requests in between are skipped.
    fn request(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!("no answer to {method} in time"))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("the server stopped before answering {method}"))
                }
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["id"] != json!(id) || message.get("method").is_some() {
                continue;
            }
            if let Some(error) = message.get("error") {
                let text = error["message"].as_str().unwrap_or("an error");
                return Err(format!("{method} failed: {}", safe_line(text, 200)));
            }
            return Ok(message["result"].clone());
        }
    }

    fn ask_tools(&mut self) -> Result<Vec<Tool>, String> {
        self.request(
            "initialize",
            &json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "harness", "version": env!("CARGO_PKG_VERSION")},
            }),
        )?;
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let result = self.request("tools/list", &params)?;
            tools.extend(parse_tools(&result));
            cursor = result["nextCursor"].as_str().map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }
}

/// The tools of one `tools/list` answer.
pub fn parse_tools(result: &Value) -> Vec<Tool> {
    result["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let name = safe_line(tool["name"].as_str()?, 200);
                    let description = tool["description"]
                        .as_str()
                        .and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty()))
                        .map(|t| safe_line(t, 200));
                    Some(Tool { name, description })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[cfg(unix)]
    use {std::fs, std::os::unix::fs::PermissionsExt};

    #[test]
    fn tools_are_read_without_control_characters() {
        let result = json!({"tools": [
            {"name": "search", "description": "Searches.\nMore lines."},
            {"name": "bad\u{1b}[2J", "description": "\n\n  Second line first "},
            {"description": "no name"},
        ]});
        let tools = parse_tools(&result);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].description.as_deref(), Some("Searches."));
        // The whole escape sequence goes, not only the ESC character.
        assert_eq!(tools[1].name, "bad");
        assert_eq!(tools[1].description.as_deref(), Some("Second line first"));
    }

    /// A tiny MCP server in shell: answers initialize and two pages of
    /// tools/list; prints its secret to stderr when asked for a third.
    #[cfg(unix)] // a shell script stands in for the program
    fn fake_server(dir: &Path, fail: bool) -> McpServer {
        let script = dir.join("server.sh");
        let last = if fail {
            r#"echo "boom $TOKEN" >&2; exit 3"#
        } else {
            r#"echo '{"jsonrpc":"2.0","id":3,"result":{"tools":[{"name":"third"}]}}'"#
        };
        fs::write(
            &script,
            format!(
                r#"#!/bin/sh
read line
echo '{{"jsonrpc":"2.0","method":"notifications/message","params":{{}}}}'
echo '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2025-06-18","capabilities":{{"tools":{{}}}}}}}}'
read line
read line
echo 'not json'
echo '{{"jsonrpc":"2.0","id":2,"result":{{"tools":[{{"name":"first","description":"One."}},{{"name":"second"}}],"nextCursor":"p2"}}}}'
read line
{last}
"#
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        McpServer {
            name: "fake".into(),
            command: script.display().to_string(),
            args: vec![],
            env: [("TOKEN".to_string(), Secret::new("tok-12345678"))].into(),
        }
    }

    #[cfg(unix)] // a shell script stands in for the program
    #[test]
    fn a_server_is_asked_for_all_pages_of_its_tools() {
        let dir = tempfile::tempdir().unwrap();
        let tools = list_tools(&fake_server(dir.path(), false), dir.path()).unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["first", "second", "third"]);
    }

    #[cfg(unix)] // a shell script stands in for the program
    #[test]
    fn a_failing_server_says_why_without_its_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let error = list_tools(&fake_server(dir.path(), true), dir.path()).unwrap_err();
        assert!(error.contains("stopped"), "{error}");
        assert!(error.contains("boom ***"), "{error}");
        assert!(!error.contains("tok-12345678"), "{error}");

        let missing = McpServer {
            name: "none".into(),
            command: "/no/such/program".into(),
            args: vec![],
            env: BTreeMap::default(),
        };
        assert!(list_tools(&missing, dir.path())
            .unwrap_err()
            .contains("cannot start"));
    }

    #[cfg(unix)] // `sleep` is a Unix program
    #[test]
    fn a_silent_server_runs_out_of_time() {
        let dir = tempfile::tempdir().unwrap();
        let server = McpServer {
            name: "silent".into(),
            command: "sleep".into(),
            args: vec!["30".into()],
            env: BTreeMap::default(),
        };
        let start = Instant::now();
        let error = list_tools_within(&server, dir.path(), Duration::from_millis(300)).unwrap_err();
        assert!(error.contains("no answer to initialize"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    /// A real server from npm: `cargo test -p harness-agents -- --ignored`.
    #[test]
    #[ignore = "downloads a server with npx"]
    fn the_everything_server_lists_its_tools() {
        let dir = tempfile::tempdir().unwrap();
        let server = McpServer {
            name: "everything".into(),
            command: "npx".into(),
            args: vec![
                "-y".into(),
                "@modelcontextprotocol/server-everything".into(),
                "stdio".into(),
            ],
            env: BTreeMap::default(),
        };
        let tools = list_tools(&server, dir.path()).unwrap();
        assert!(tools.iter().any(|t| t.name == "echo"), "{tools:?}");
    }
}
