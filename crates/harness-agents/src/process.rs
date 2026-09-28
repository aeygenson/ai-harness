//! What every console agent adapter shares: a clean environment, starting the
//! program with the prompt on standard input, the time-out, and the log.

use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use harness_core::agent::AgentOutcome;
use tokio::io::AsyncWriteExt;

/// Environment variables an agent may inherit from Lisa's terminal. Everything
/// else, including every `*_API_KEY`, is removed.
pub const INHERITED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "LC_ALL",
    "TERM",
    "TMPDIR",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];

/// `program`, started in the project folder with an empty environment plus
/// the whitelist. Each adapter adds its own variables and flags.
pub fn base_command(program: &Path, project_dir: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(project_dir).env_clear().envs(
        INHERITED_ENV
            .iter()
            .filter_map(|name| std::env::var_os(name).map(|value| (*name, value))),
    );
    command
}

/// What a finished agent printed.
#[derive(Debug)]
pub struct Finished {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

/// Starts the command, writes `prompt` to its standard input and waits at most
/// `timeout`. `Err` holds a short explanation for Lisa.
pub async fn run(command: Command, prompt: &str, timeout: Duration) -> Result<Finished, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut command = tokio::process::Command::from(command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // If we stop waiting (time-out), the agent is killed too.
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // An agent may exit without reading everything; that is not our problem.
        let _ = stdin.write_all(prompt.as_bytes()).await;
    }
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(Finished {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        }),
        Ok(Err(e)) => Err(format!("error while waiting for the agent: {e}")),
        Err(_) => Err(format!(
            "timed out after {} s; the agent was stopped",
            timeout.as_secs()
        )),
    }
}

/// Adds the agent's output to the log.
pub fn append_output(log: &mut String, finished: &Finished) {
    log.push_str(&finished.stdout);
    if !finished.stderr.trim().is_empty() {
        log.push_str("\n--- stderr ---\n");
        log.push_str(&finished.stderr);
    }
}

/// Adds `message` to the log too, and returns a failed outcome.
pub fn failed(mut log: String, message: String) -> AgentOutcome {
    log.push_str(&message);
    log.push('\n');
    AgentOutcome {
        success: false,
        usage_limit_reached: false,
        log,
        message,
    }
}

/// Agents report a used-up subscription only as text, so we look for the usual
/// phrases. Pausing by mistake is harmless: Lisa just runs again.
pub fn looks_like_usage_limit(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "usage limit",
        "hit your limit",
        "limit reached",
        "rate limit",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_limit_phrases_are_recognised() {
        assert!(looks_like_usage_limit(
            "Claude AI usage limit reached|1759000000"
        ));
        assert!(looks_like_usage_limit("You've hit your limit · resets 5pm"));
        assert!(!looks_like_usage_limit("All tests pass."));
    }
}
