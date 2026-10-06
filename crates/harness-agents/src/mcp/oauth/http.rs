//! HTTP requests with `curl` for the sign-in, and the token requests built on them.

use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::Stdio;

use harness_core::mcp::is_allowed_url;
use harness_core::text::safe_line;
use serde_json::Value;

use super::encoding::form_encode;
use crate::process::base_command;

/// What a token endpoint gave.
pub(super) struct Tokens {
    pub(super) access_token: String,
    pub(super) refresh_token: Option<String>,
    pub(super) expires_in: Option<u64>,
}

pub(super) fn token_request(
    curl: &Path,
    endpoint: &str,
    form: &[(&str, String)],
) -> Result<Tokens, String> {
    let answer = http(
        curl,
        "POST",
        endpoint,
        &["Accept: application/json"],
        Some(("application/x-www-form-urlencoded", &form_encode(form))),
    )?;
    if !(200..300).contains(&answer.code) {
        return Err(format!(
            "the token request failed: HTTP {}{}",
            answer.code,
            said(&answer.body)
        ));
    }
    let tokens: Value =
        serde_json::from_str(&answer.body).map_err(|_| "the token answer is not JSON")?;
    let access_token = tokens["access_token"]
        .as_str()
        .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
        .ok_or("the token answer has no access_token")?;
    Ok(Tokens {
        access_token: access_token.to_string(),
        refresh_token: tokens["refresh_token"].as_str().map(str::to_string),
        expires_in: tokens["expires_in"]
            .as_u64()
            .or_else(|| tokens["expires_in"].as_str()?.parse().ok()),
    })
}

/// The `error` and `error_description` of an OAuth error answer, if any.
pub(super) fn said(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return String::new();
    };
    let text = [&value["error"], &value["error_description"]]
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(": ");
    if text.is_empty() {
        String::new()
    } else {
        format!(" ({})", safe_line(&text, 300))
    }
}

/// An HTTP answer.
pub(super) struct Answer {
    pub(super) code: u16,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: String,
}

impl Answer {
    pub(super) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

pub(super) fn get_json(curl: &Path, address: &str) -> Option<Value> {
    let answer = http(curl, "GET", address, &["Accept: application/json"], None).ok()?;
    if !(200..300).contains(&answer.code) {
        return None;
    }
    serde_json::from_str(&answer.body).ok()
}

/// One request with curl. The body goes on curl's standard input, so the
/// codes and tokens in it are never on a command line.
pub(super) fn http(
    curl: &Path,
    method: &str,
    address: &str,
    headers: &[&str],
    body: Option<(&str, &str)>,
) -> Result<Answer, String> {
    if !is_allowed_url(address) {
        return Err(format!("{address:?} is not https"));
    }
    let dir = std::env::temp_dir();
    let mut command = base_command(curl, &dir);
    let protocols = if address.starts_with("http://") {
        "=http,https"
    } else {
        "=https"
    };
    command
        .args(["-sS", "-i", "-m", "30", "--max-filesize", "1000000"])
        .args(["--noproxy", "localhost,127.0.0.1,::1", "--proto", protocols])
        .args(["-X", method]);
    for header in headers {
        command.arg("-H").arg(header);
    }
    if let Some((content_type, _)) = body {
        command
            .arg("-H")
            .arg(format!("Content-Type: {content_type}"))
            .args(["--data-binary", "@-"]);
    }
    command
        .arg(address)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child =
        crate::process::spawn(&mut command).map_err(|e| format!("cannot start curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some((_, text)) = body {
            let _ = stdin.write_all(text.as_bytes());
        }
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stdout.as_bytes());
    match crate::mcp::http_head::read_head(&mut reader) {
        Some((code, headers)) => {
            let mut body = String::new();
            let _ = reader.read_to_string(&mut body);
            Ok(Answer {
                code,
                headers,
                body,
            })
        }
        None => {
            let why = stderr.lines().map(str::trim).rfind(|l| !l.is_empty());
            Err(safe_line(
                &format!(
                    "{address}: {}",
                    why.unwrap_or(&format!("no answer ({status})"))
                ),
                300,
            ))
        }
    }
}
