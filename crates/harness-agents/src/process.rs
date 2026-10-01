//! What every console agent adapter shares: a clean environment, starting the
//! program with the prompt on standard input, the time-out, and the log.

use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

use harness_core::agent::AgentOutcome;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};

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

/// Starts `command`. A program file that another thread has just written can
/// be «busy» for a moment (Linux `ETXTBSY`, seen when tests write fake
/// programs in parallel); that is tried again a few times.
pub fn spawn(command: &mut Command) -> std::io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

/// What a finished agent printed.
#[derive(Debug)]
pub struct Finished {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

/// Starts the command, writes `prompt` to its standard input and waits at most
/// `timeout`. Each line the agent prints also goes to the live log (see
/// [`set_live_log`]), with `secrets` hidden. `Err` holds a short explanation
/// for Lisa.
pub async fn run(
    command: Command,
    prompt: &str,
    timeout: Duration,
    secrets: &[&str],
) -> Result<Finished, String> {
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
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Both streams are read line by line while the agent works, so the live
    // log shows its progress and a full pipe never blocks it.
    let work = async {
        let (stdout, stderr, status) = tokio::join!(
            read_lines(stdout, secrets),
            read_lines(stderr, secrets),
            child.wait()
        );
        Ok::<_, std::io::Error>((stdout?, stderr?, status?))
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(Ok((stdout, stderr, status))) => Ok(Finished {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            status,
        }),
        Ok(Err(e)) => Err(format!("error while waiting for the agent: {e}")),
        Err(_) => Err(format!(
            "timed out after {} s; the agent was stopped",
            timeout.as_secs()
        )),
    }
}

/// Everything `stream` gives, sending each line to the live log on the way.
async fn read_lines<R: AsyncRead + Unpin>(
    stream: Option<R>,
    secrets: &[&str],
) -> std::io::Result<Vec<u8>> {
    let Some(stream) = stream else {
        return Ok(Vec::new());
    };
    let mut reader = BufReader::new(stream);
    let mut all = Vec::new();
    loop {
        let start = all.len();
        if reader.read_until(b'\n', &mut all).await? == 0 {
            return Ok(all);
        }
        send_live(&all[start..], secrets);
    }
}

/// Where the live log goes: the TUI shows what the running agent prints.
static LIVE_LOG: Mutex<Option<Sender<String>>> = Mutex::new(None);

/// From now on every line an agent prints is also sent to `sink`, with the
/// agent's secrets hidden; `None` stops it. The command line never sets it.
pub fn set_live_log(sink: Option<Sender<String>>) {
    if let Ok(mut live) = LIVE_LOG.lock() {
        *live = sink;
    }
}

fn send_live(line: &[u8], secrets: &[&str]) {
    let Ok(mut live) = LIVE_LOG.lock() else {
        return;
    };
    let Some(sink) = live.as_ref() else {
        return;
    };
    let line = hide(
        String::from_utf8_lossy(line).trim_end().to_string(),
        secrets,
    );
    if sink.send(line).is_err() {
        // Nobody listens any more.
        *live = None;
    }
}

/// `text` with every secret replaced by `***`. A very short value would hide
/// ordinary words too; real keys are long.
fn hide(mut text: String, secrets: &[&str]) -> String {
    for secret in secrets.iter().filter(|s| s.len() >= 8) {
        text = text.replace(secret, "***");
    }
    text
}

/// Adds the agent's output to the log.
pub fn append_output(log: &mut String, finished: &Finished) {
    log.push_str(&finished.stdout);
    if !finished.stderr.trim().is_empty() {
        log.push_str("\n--- stderr ---\n");
        log.push_str(&finished.stderr);
    }
}

/// Replaces every secret the agent was given with `***` in what it printed.
/// Its output becomes `agent.log`, which is committed to git, and an MCP tool
/// may echo a key back (a tool that prints its environment does exactly that).
pub fn hide_secrets<'a>(
    mut result: Result<Finished, String>,
    secrets: impl IntoIterator<Item = &'a str>,
) -> Result<Finished, String> {
    let secrets: Vec<&str> = secrets.into_iter().collect();
    match &mut result {
        Ok(finished) => {
            finished.stdout = hide(std::mem::take(&mut finished.stdout), &secrets);
            finished.stderr = hide(std::mem::take(&mut finished.stderr), &secrets);
        }
        Err(message) => *message = hide(std::mem::take(message), &secrets),
    }
    result
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
        // Google: "You have exhausted your daily quota", RESOURCE_EXHAUSTED.
        "exhausted your",
        "resource_exhausted",
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
        assert!(looks_like_usage_limit(
            "AGY_ERROR: {\"status\":\"RESOURCE_EXHAUSTED\"}"
        ));
        assert!(!looks_like_usage_limit("All tests pass."));
    }

    #[tokio::test]
    async fn lines_reach_the_live_log_with_secrets_hidden() {
        let (sink, lines) = std::sync::mpsc::channel();
        set_live_log(Some(sink));
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "read x; echo \"live-test $x key-12345678\"; echo live-test err >&2",
        ]);
        let finished = run(
            command,
            "hello\n",
            Duration::from_secs(10),
            &["key-12345678"],
        )
        .await
        .unwrap();
        set_live_log(None);

        // The whole output is still returned; hiding it is `hide_secrets`' job.
        assert_eq!(finished.stdout, "live-test hello key-12345678\n");
        assert_eq!(finished.stderr, "live-test err\n");
        // Other tests may run agents at the same time; keep only ours.
        let mut ours: Vec<String> = lines
            .try_iter()
            .filter(|l: &String| l.starts_with("live-test"))
            .collect();
        ours.sort();
        assert_eq!(ours, ["live-test err", "live-test hello ***"]);
    }

    #[tokio::test]
    async fn a_slow_agent_is_stopped() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 5"]);
        let error = run(command, "", Duration::from_millis(200), &[])
            .await
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
    }
}
