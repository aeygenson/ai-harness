//! One request of the bridge: sending a message with `curl` and passing on the answers,
//! plain JSON or a stream of events.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::Stdio;
use std::thread;

use serde_json::Value;

use super::Shared;
use crate::mcp::http_head::read_head;

impl Shared {
    /// The ids of the answers the server sent.
    pub(super) fn exchange(&self, message: &Value, initialize: bool) -> Result<Vec<Value>, String> {
        let mut command = self.curl("POST");
        command
            .args(["--data-binary", "@-"])
            .arg(&self.bridge.url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child =
            crate::process::spawn(&mut command).map_err(|e| format!("cannot start curl: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(message.to_string().as_bytes());
        }
        let stderr = child.stderr.take().map(|mut pipe| {
            thread::spawn(move || {
                let mut text = String::new();
                let _ = pipe.read_to_string(&mut text);
                text
            })
        });
        let mut answered = Vec::new();
        let result = match child.stdout.take() {
            Some(stdout) => self.read_answer(BufReader::new(stdout), initialize, &mut answered),
            None => Err("no output from curl".into()),
        };
        let status = child.wait();
        let stderr = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
        match result {
            Ok(()) => Ok(answered),
            Err(why) => {
                let said = stderr.lines().map(str::trim).rfind(|l| !l.is_empty());
                match (said, status) {
                    (Some(said), Ok(status)) if !status.success() => Err(said.to_string()),
                    _ => Err(why),
                }
            }
        }
    }

    fn read_answer(
        &self,
        mut reader: impl BufRead,
        initialize: bool,
        answered: &mut Vec<Value>,
    ) -> Result<(), String> {
        let (code, headers) = read_head(&mut reader).ok_or("the server did not answer")?;
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        if initialize {
            if let Some(session) = header("mcp-session-id")
                .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_graphic()))
            {
                if let Ok(mut current) = self.session.lock() {
                    *current = Some(session.to_string());
                }
            }
        }
        if !(200..300).contains(&code) {
            let hint = match code {
                401 | 403 => " (check the key: harness secret set ...)",
                404 if !initialize => " (the session may have ended)",
                _ => "",
            };
            return Err(format!("the server answered HTTP {code}{hint}"));
        }
        let mut deliver = |text: &str| {
            let Ok(value) = serde_json::from_str::<Value>(text) else {
                return;
            };
            let messages = match value {
                Value::Array(messages) => messages,
                one => vec![one],
            };
            for message in messages {
                if message.get("method").is_none() {
                    if let Some(id) = message.get("id") {
                        answered.push(id.clone());
                    }
                }
                if initialize {
                    if let Some(version) = message["result"]["protocolVersion"].as_str() {
                        if version.chars().all(|c| c.is_ascii_graphic()) {
                            if let Ok(mut current) = self.protocol.lock() {
                                *current = Some(version.to_string());
                            }
                        }
                    }
                }
                self.write(&message);
            }
        };
        let events = header("content-type")
            .is_some_and(|t| t.to_ascii_lowercase().starts_with("text/event-stream"));
        if events {
            let mut data = String::new();
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let line = line.trim_end_matches('\r');
                if line.is_empty() {
                    if !data.is_empty() {
                        deliver(&data);
                        data.clear();
                    }
                } else if let Some(part) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(part.strip_prefix(' ').unwrap_or(part));
                }
            }
            if !data.is_empty() {
                deliver(&data);
            }
        } else {
            let mut body = String::new();
            let _ = reader.read_to_string(&mut body);
            if !body.trim().is_empty() {
                deliver(body.trim());
            }
        }
        Ok(())
    }
}
