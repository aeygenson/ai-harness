//! `harness mcp-remote`: the bridge to an MCP server on the web.
//!
//! The agent starts it like any program server and talks MCP on its standard
//! input and output, one JSON message per line. The bridge sends each message
//! to the server's address (MCP «streamable HTTP»: a POST that is answered with
//! JSON or with a stream of events) and writes the answers back.
//!
//! The address and the headers come from the variables the harness gives the
//! bridge (`HARNESS_MCP_URL`, `HARNESS_MCP_HEADER_<n>`, see
//! `harness_core::mcp`). The headers carry the keys, so the agent never sees
//! them: they are written to a private file that `curl` reads (`-H @file`),
//! never put on a command line, and they are hidden in every error.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use harness_core::mcp::{is_allowed_url, BRIDGE_HEADER, BRIDGE_URL};
use serde_json::{json, Value};

use crate::credentials::write_private;
use crate::process::base_command;

/// How long one request may take; a tool may work for a while.
pub const REQUEST_LIMIT: Duration = Duration::from_secs(600);

/// Where the bridge sends the messages.
#[derive(Clone)]
pub struct Bridge {
    pub url: String,
    /// `Name: value`, with the secrets already in.
    pub headers: Vec<String>,
    pub curl: PathBuf,
    pub limit: Duration,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("url", &self.url)
            .field("headers", &self.headers.len())
            .finish_non_exhaustive()
    }
}

impl Bridge {
    /// The bridge the harness described in the variables.
    pub fn from_env() -> Result<Self, String> {
        let url = std::env::var(BRIDGE_URL).map_err(|_| format!("{BRIDGE_URL} is not set"))?;
        if !is_allowed_url(&url) {
            return Err(format!("{url:?} is not an address the bridge may use"));
        }
        let headers = (1..)
            .map_while(|n| std::env::var(format!("{BRIDGE_HEADER}{n}")).ok())
            .collect();
        Ok(Self {
            url,
            headers,
            curl: PathBuf::from("curl"),
            limit: REQUEST_LIMIT,
        })
    }

    /// Passes messages until `input` ends, then ends the server's session.
    pub fn run<W: Write + Send + 'static>(&self, input: impl BufRead, output: W) -> io::Result<()> {
        let dir = tempfile::Builder::new()
            .prefix("harness-remote-")
            .tempdir()?;
        let header_file = dir.path().join("headers");
        let mut lines = String::from(
            "Content-Type: application/json\nAccept: application/json, text/event-stream\n",
        );
        for header in &self.headers {
            lines.push_str(header);
            lines.push('\n');
        }
        write_private(&header_file, &lines)?;
        let shared = Arc::new(Shared {
            bridge: self.clone(),
            dir: dir.path().to_path_buf(),
            header_file,
            output: Mutex::new(Box::new(output)),
            session: Mutex::new(None),
            protocol: Mutex::new(None),
        });
        let mut running = Vec::new();
        for line in input.lines() {
            let line = line?;
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["method"] == "initialize" {
                // Its answer brings the session the other messages need.
                shared.post(&message);
            } else {
                let shared = Arc::clone(&shared);
                running.push(thread::spawn(move || shared.post(&message)));
            }
        }
        for handle in running {
            let _ = handle.join();
        }
        shared.end_session();
        Ok(())
    }
}

/// What the requests share.
struct Shared {
    bridge: Bridge,
    /// The private temporary folder.
    dir: PathBuf,
    header_file: PathBuf,
    output: Mutex<Box<dyn Write + Send>>,
    /// `Mcp-Session-Id` from the answer to `initialize`.
    session: Mutex<Option<String>>,
    /// The protocol version the server chose.
    protocol: Mutex<Option<String>>,
}

impl Shared {
    fn write(&self, message: &Value) {
        if let Ok(mut output) = self.output.lock() {
            let _ = writeln!(output, "{message}");
            let _ = output.flush();
        }
    }

    /// The text without the secrets of the headers and without control
    /// characters.
    fn hide(&self, text: &str) -> String {
        let mut text: String = text.chars().filter(|c| !c.is_control()).take(300).collect();
        for header in &self.bridge.headers {
            let value = header.split_once(':').map_or("", |(_, v)| v.trim());
            if !value.is_empty() {
                text = text.replace(value, "***");
            }
            // `Bearer <key>`: the key alone, too.
            if let Some(key) = value.split_whitespace().last().filter(|k| k.len() >= 8) {
                text = text.replace(key, "***");
            }
        }
        text
    }

    fn curl(&self, method: &str) -> Command {
        let bridge = &self.bridge;
        let mut command = base_command(&bridge.curl, &self.dir);
        let protocols = if bridge.url.starts_with("http://") {
            "=http,https"
        } else {
            "=https"
        };
        command
            .args(["-sS", "-i", "-N", "-m"])
            .arg(bridge.limit.as_secs().max(1).to_string())
            .args(["--noproxy", "localhost,127.0.0.1,::1", "--proto", protocols])
            .args(["-X", method, "-H"])
            .arg(format!("@{}", self.header_file.display()));
        if let Some(session) = self.session.lock().ok().and_then(|s| s.clone()) {
            command.arg("-H").arg(format!("Mcp-Session-Id: {session}"));
        }
        if let Some(version) = self.protocol.lock().ok().and_then(|p| p.clone()) {
            command
                .arg("-H")
                .arg(format!("MCP-Protocol-Version: {version}"));
        }
        command
    }

    /// Sends one message and writes what the server answers. A request that
    /// gets no answer is answered with an error, so the agent does not wait.
    fn post(&self, message: &Value) {
        let id = message
            .get("id")
            .filter(|_| message.get("method").is_some())
            .cloned();
        let initialize = message["method"] == "initialize";
        let answered = match self.exchange(message, initialize) {
            Ok(answered) => answered,
            Err(why) => {
                if let Some(id) = &id {
                    self.write(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32000, "message": self.hide(&why)},
                    }));
                }
                return;
            }
        };
        if let Some(id) = id.filter(|id| !answered.contains(id)) {
            self.write(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32000, "message": "the server sent no answer"},
            }));
        }
    }

    /// The ids of the answers the server sent.
    fn exchange(&self, message: &Value, initialize: bool) -> Result<Vec<Value>, String> {
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

    /// Tells the server the session is over; it may not care.
    fn end_session(&self) {
        if self.session.lock().ok().and_then(|s| s.clone()).is_none() {
            return;
        }
        let mut command = self.curl("DELETE");
        command
            .args(["-o", "/dev/null", "--max-time", "10"])
            .arg(&self.bridge.url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = command.status();
    }
}

/// The status and headers of an HTTP answer, as `curl -i` prints them. The
/// proxy's «Connection established» and `100 Continue` come first and are
/// skipped.
pub(crate) fn read_head(reader: &mut impl BufRead) -> Option<(u16, Vec<(String, String)>)> {
    loop {
        let mut status = String::new();
        if reader.read_line(&mut status).ok()? == 0 {
            return None;
        }
        let status = status.trim();
        if status.is_empty() {
            continue;
        }
        let code: u16 = status.split_whitespace().nth(1)?.parse().ok()?;
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 {
                break;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_string(), value.trim().to_string()));
            }
        }
        let tunnel = status
            .to_ascii_lowercase()
            .contains("connection established");
        if (100..200).contains(&code) || tunnel {
            continue;
        }
        return Some((code, headers));
    }
}

/// `harness mcp-remote`: the bridge between standard input and output and the
/// server the variables describe.
pub fn run_from_env() -> Result<(), String> {
    let bridge = Bridge::from_env()?;
    let stdin = io::stdin();
    bridge
        .run(stdin.lock(), io::stdout())
        .map_err(|e| format!("the MCP bridge stopped: {e}"))
}

/// `curl` as the bridge uses it; tests give another.
pub fn default_curl() -> &'static Path {
    Path::new("curl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::TcpListener;

    /// What the fake server saw: method line, headers, body.
    type Seen = Arc<Mutex<Vec<(String, Vec<(String, String)>, String)>>>;

    /// A tiny MCP server on the web: initialize gives a session, tools/list
    /// answers with events, tools/call fails, a wrong key gets 401.
    fn fake_server() -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://127.0.0.1:{}/mcp",
            listener.local_addr().unwrap().port()
        );
        let seen: Seen = Arc::default();
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let log = Arc::clone(&log);
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request = String::new();
                    reader.read_line(&mut request).unwrap();
                    let mut headers = Vec::new();
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        let line = line.trim_end().to_string();
                        if line.is_empty() {
                            break;
                        }
                        let (name, value) = line.split_once(':').unwrap();
                        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
                    }
                    let get = |name: &str| {
                        headers
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default()
                    };
                    let length: usize = get("content-length").parse().unwrap_or(0);
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let body = String::from_utf8(body).unwrap();
                    log.lock().unwrap().push((
                        request.trim().to_string(),
                        headers.clone(),
                        body.clone(),
                    ));
                    let message: Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = message["id"].clone();
                    let answer = if get("authorization") != "Bearer tok-12345678" {
                        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_string()
                    } else if request.starts_with("DELETE") {
                        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
                    } else if message["method"] == "initialize" {
                        let body = json!({"jsonrpc": "2.0", "id": id, "result": {
                            "protocolVersion": "2025-06-18", "capabilities": {"tools": {}}}})
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                             Mcp-Session-Id: s-1\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                    } else if get("mcp-session-id") != "s-1"
                        || get("mcp-protocol-version") != "2025-06-18"
                    {
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n".to_string()
                    } else if message.get("id").is_none() {
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n".to_string()
                    } else if message["method"] == "tools/list" {
                        let note = json!({"jsonrpc": "2.0", "method": "notifications/message",
                            "params": {"level": "info", "data": "listing"}});
                        let result = json!({"jsonrpc": "2.0", "id": id,
                            "result": {"tools": [{"name": "search"}]}});
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                             Connection: close\r\n\r\n: comment\r\nevent: message\r\n\
                             data: {note}\r\n\r\ndata: {result}\r\n\r\n"
                        )
                    } else {
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n"
                            .to_string()
                    };
                    let mut stream = stream;
                    let _ = stream.write_all(answer.as_bytes());
                });
            }
        });
        (url, seen)
    }

    /// The output of the bridge, kept for the test to read.
    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<u8>>>);

    impl Write for Output {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Output {
        fn messages(&self) -> Vec<Value> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
    }

    fn bridge(url: &str, key: &str) -> Bridge {
        Bridge {
            url: url.to_string(),
            headers: vec![format!("Authorization: Bearer {key}")],
            curl: default_curl().to_path_buf(),
            limit: Duration::from_secs(20),
        }
    }

    const MESSAGES: &str = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\nnot json\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":"three","method":"tools/call","params":{"name":"x"}}"#,
        "\n"
    );

    #[test]
    fn messages_go_to_the_server_and_its_answers_come_back() {
        let (url, seen) = fake_server();
        let output = Output::default();
        bridge(&url, "tok-12345678")
            .run(Cursor::new(MESSAGES), output.clone())
            .unwrap();
        let messages = output.messages();
        let by_id = |id: Value| {
            messages
                .iter()
                .find(|m| m["id"] == id && m.get("method").is_none())
                .unwrap_or_else(|| panic!("no answer to {id}: {messages:?}"))
        };
        assert_eq!(by_id(json!(1))["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(by_id(json!(2))["result"]["tools"][0]["name"], "search");
        let error = by_id(json!("three"))["error"]["message"].as_str().unwrap();
        assert!(error.contains("HTTP 500"), "{error}");
        assert!(messages
            .iter()
            .any(|m| m["method"] == "notifications/message"));
        // Nothing for the notification, which the server only accepted.
        assert_eq!(messages.len(), 4, "{messages:?}");

        let seen = seen.lock().unwrap();
        assert!(seen.iter().any(|(r, _, _)| r.starts_with("DELETE")));
        let first = &seen[0];
        assert_eq!(
            first.1.iter().find(|(n, _)| n == "accept").unwrap().1,
            "application/json, text/event-stream"
        );
    }

    #[test]
    fn a_wrong_key_is_an_error_without_the_key() {
        let (url, _) = fake_server();
        let output = Output::default();
        bridge(&url, "wrong-key-987654")
            .run(Cursor::new(MESSAGES), output.clone())
            .unwrap();
        let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("HTTP 401"), "{text}");
        assert!(!text.contains("wrong-key"), "{text}");
    }

    #[test]
    fn the_key_is_never_on_curls_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let curl = dir.path().join("curl");
        let log = dir.path().join("args");
        std::fs::write(
            &curl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\necho 'curl: (7) refused tok-12345678' >&2\nexit 7\n",
                log.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut bridge = bridge("https://example.com/mcp", "tok-12345678");
        bridge.curl = curl;
        let output = Output::default();
        bridge.run(Cursor::new(MESSAGES), output.clone()).unwrap();
        let args = std::fs::read_to_string(&log).unwrap();
        assert!(!args.contains("tok-12345678"), "{args}");
        assert!(args.contains("=https"), "{args}");
        let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("refused ***"), "{text}");
        assert!(!text.contains("tok-12345678"), "{text}");
    }

    #[test]
    fn proxy_and_continue_heads_are_skipped() {
        let answer = "HTTP/1.1 200 Connection established\r\n\r\n\
                      HTTP/1.1 100 Continue\r\n\r\n\
                      HTTP/2 202 \r\nmcp-session-id: x\r\n\r\n";
        let (code, headers) = read_head(&mut Cursor::new(answer)).unwrap();
        assert_eq!(code, 202);
        assert_eq!(headers, [("mcp-session-id".to_string(), "x".to_string())]);
    }
}
