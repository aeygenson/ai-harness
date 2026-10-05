//! What every console agent adapter shares: a clean environment, starting the
//! program with the prompt on standard input, the time-out, and the log.

use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

use harness_core::agent::AgentOutcome;
use harness_core::secret;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};

/// `program`, started in the project folder with an empty environment plus
/// the whitelist. Each adapter adds its own variables and flags.
pub fn base_command(program: &Path, project_dir: &Path) -> Command {
    let mut command = Command::new(harness_platform::program::resolve(program));
    // The whitelist (`harness_platform::env`): nothing that holds a secret.
    command
        .current_dir(project_dir)
        .env_clear()
        .envs(harness_platform::env::inherited_values());
    command
}

/// Starts `command`. A program file that another thread has just written can
/// be «busy» for a moment (`ETXTBSY` on Linux and macOS, seen when tests write fake
/// programs in parallel); that is tried again a few times.
pub fn spawn(command: &mut Command) -> std::io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy && tries < 20 => {
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
///
/// The agent runs in its own process group. When it ends, times out, or the
/// harness stops waiting (Ctrl+C), everything it started is stopped too: a
/// test run, a build or a server it left in the background.
pub async fn run(
    mut command: Command,
    prompt: &str,
    timeout: Duration,
    secrets: &[&str],
) -> Result<Finished, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    harness_platform::process::own_group(&mut command);
    let mut command = tokio::process::Command::from(command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A safety net: the agent itself is killed even if the tree kill fails.
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    // From here on, leaving this function in any way stops the whole tree.
    let tree = child.id().map(ProcessTree);
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // The prompt is written, both streams are read and the agent is awaited
    // all at the same time, and all inside the time limit: an agent that never
    // reads a long prompt cannot block the harness.
    let work = async {
        let (_, stdout, stderr, status) = tokio::join!(
            write_prompt(stdin, prompt),
            read_lines(stdout, secrets),
            read_lines(stderr, secrets),
            async {
                let status = child.wait().await;
                // Something the agent left in the background may still hold
                // its output open; stopping it lets the reading above finish.
                if let Some(tree) = &tree {
                    tree.stop();
                }
                status
            }
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

/// The process group of a running agent. Dropping it stops the whole group,
/// so it is stopped on every way out of [`run`], including when the caller
/// stops waiting (the future is dropped).
struct ProcessTree(u32);

impl ProcessTree {
    fn stop(&self) {
        harness_platform::process::kill_tree_of(self.0);
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Writes the prompt and closes standard input, so the agent knows it is all.
async fn write_prompt(stdin: Option<tokio::process::ChildStdin>, prompt: &str) {
    if let Some(mut stdin) = stdin {
        // An agent may exit without reading everything; that is not our problem.
        let _ = stdin.write_all(prompt.as_bytes()).await;
        // `stdin` is dropped here, which closes the pipe.
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
    let line = secret::hide(String::from_utf8_lossy(line).trim_end(), secrets);
    if sink.send(line).is_err() {
        // Nobody listens any more.
        *live = None;
    }
}

/// `text` with every secret replaced by `***`. A very short value would hide
/// ordinary words too; real keys are long.
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
            finished.stdout = secret::hide(&finished.stdout, &secrets);
            finished.stderr = secret::hide(&finished.stderr, &secrets);
        }
        Err(message) => *message = secret::hide(message, &secrets),
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

    #[cfg(unix)] // `sh` stands in for an agent
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

    /// Is the process with the id written in `pid_file` still running? A
    /// stopped process can take a moment to disappear, so this waits a little.
    #[cfg(unix)]
    fn still_running(pid_file: &Path) -> bool {
        let pid = std::fs::read_to_string(pid_file).unwrap();
        for _ in 0..40 {
            let alive = Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            if !alive {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        true
    }

    /// An agent that starts `sleep 30` in the background and writes its id.
    #[cfg(unix)]
    fn agent_with_a_helper(pid_file: &Path, then: &str) -> Command {
        let mut command = Command::new("sh");
        let script = format!("sleep 30 & echo $! > '{}'; {then}", pid_file.display());
        command.args(["-c", &script]);
        command
    }

    #[cfg(unix)] // `sh` stands in for an agent
    #[tokio::test]
    async fn what_a_finished_agent_left_running_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let command = agent_with_a_helper(&pid_file, "echo done");
        let start = std::time::Instant::now();

        // The helper keeps the agent's output open; without stopping it the
        // reading would wait for the whole time limit.
        let finished = run(command, "", Duration::from_secs(20), &[])
            .await
            .unwrap();

        assert_eq!(finished.stdout, "done\n");
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(!still_running(&pid_file));
    }

    #[cfg(unix)] // `sh` stands in for an agent
    #[tokio::test]
    async fn a_slow_agent_is_stopped_with_everything_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let command = agent_with_a_helper(&pid_file, "wait");

        let error = run(command, "", Duration::from_millis(500), &[])
            .await
            .unwrap_err();

        assert!(error.contains("timed out"), "{error}");
        assert!(!still_running(&pid_file));
    }

    #[cfg(unix)] // `sh` stands in for an agent
    #[tokio::test]
    async fn when_the_harness_stops_waiting_the_agent_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let command = agent_with_a_helper(&pid_file, "wait");

        // Like Ctrl+C in the command line: the run is dropped half-way.
        tokio::select! {
            _ = run(command, "", Duration::from_secs(20), &[]) => panic!("ended by itself"),
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }

        assert!(!still_running(&pid_file));
    }

    #[cfg(unix)] // `sh` stands in for an agent
    #[tokio::test]
    async fn a_long_prompt_the_agent_never_reads_does_not_block() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        // Far more than a pipe holds (64 KB on Linux).
        let prompt = "x".repeat(1_000_000);
        let start = std::time::Instant::now();

        let error = run(command, &prompt, Duration::from_millis(300), &[])
            .await
            .unwrap_err();

        assert!(error.contains("timed out"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)] // `sh` stands in for an agent
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
