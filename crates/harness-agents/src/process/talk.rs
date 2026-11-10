//! Running an agent that is talked to line by line (the Agent Client
//! Protocol, see `adapters::acp`) instead of being given one prompt.
//!
//! Everything else is as in [`run`](super::run): the clean environment, the
//! time limit, «Stop now», the live log and stopping everything the agent
//! started.

use std::future::Future;
use std::io;
use std::process::Command;
use std::time::Duration;

use harness_core::task::stop_request::StopRequest;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use super::live_log::send_live;
use super::time_limit::time_limit_passed;
use super::{read_lines, start, stop_now_pressed, Finished, STOPPED_BY_USER, TIMED_OUT};

/// How long an agent may take to exit by itself once the conversation is
/// over and its standard input is closed. An agent that keeps running after
/// that is stopped: its work is done, and waiting longer only delays the role.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// The agent's standard input: the harness writes one message per line.
#[derive(Debug)]
pub struct AgentStdin(ChildStdin);

impl AgentStdin {
    /// Writes `line` and a line end, and sends it on at once.
    pub async fn send(&mut self, line: &str) -> io::Result<()> {
        self.0.write_all(line.as_bytes()).await?;
        self.0.write_all(b"\n").await?;
        self.0.flush().await
    }
}

/// The agent's standard output, read line by line. Every line is kept (it
/// becomes the role's log) and sent to the live log with the secrets hidden.
#[derive(Debug)]
pub struct AgentLines<'a> {
    reader: BufReader<ChildStdout>,
    all: Vec<u8>,
    secrets: &'a [&'a str],
}

impl AgentLines<'_> {
    /// The next line without its line end; `None` once the agent closed its
    /// output (usually because it ended).
    pub async fn next(&mut self) -> io::Result<Option<String>> {
        let start = self.all.len();
        if self.reader.read_until(b'\n', &mut self.all).await? == 0 {
            return Ok(None);
        }
        let line = &self.all[start..];
        send_live(line, self.secrets);
        let text = String::from_utf8_lossy(line);
        Ok(Some(text.trim_end_matches(['\r', '\n']).to_string()))
    }
}

/// Starts `command` and lets `talk` hold the conversation: it gets the
/// agent's input and output and gives the output back with what it found
/// out. When `talk` is done, the agent's input is closed and the agent gets
/// [`EXIT_GRACE`] to end before it is stopped. The time limit and «Stop now»
/// stop it as in [`run`](super::run), with the same messages.
///
/// `Err` holds a short explanation for the user; a conversation that went
/// wrong is `talk`'s own result (`T`).
pub async fn run_talking<'s, T, F, Fut>(
    command: Command,
    timeout: Duration,
    secrets: &'s [&'s str],
    stop: &StopRequest,
    talk: F,
) -> Result<(Finished, T), String>
where
    F: FnOnce(AgentStdin, AgentLines<'s>) -> Fut,
    Fut: Future<Output = (AgentLines<'s>, T)>,
{
    let (mut child, tree) = start(command, stop)?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err("the agent's input and output could not be opened".into());
    };
    let stderr = child.stderr.take();
    let lines = AgentLines {
        reader: BufReader::new(stdout),
        all: Vec::new(),
        secrets,
    };
    let work = async {
        let conversation = async {
            // `talk` owns the input, so it is closed when `talk` ends.
            let (lines, found) = talk(AgentStdin(stdin), lines).await;
            let status = match tokio::time::timeout(EXIT_GRACE, child.wait()).await {
                Ok(status) => status,
                Err(_still_running) => {
                    if let Some(tree) = &tree {
                        tree.stop();
                    }
                    child.wait().await
                }
            };
            // Whatever the agent left in the background is stopped too.
            if let Some(tree) = &tree {
                tree.stop();
            }
            (lines.all, found, status)
        };
        let ((stdout, found, status), stderr) =
            tokio::join!(conversation, read_lines(stderr, secrets));
        Ok::<_, io::Error>((stdout, found, stderr?, status?))
    };
    // As in `run`: whichever comes first wins, and dropping `tree` on the way
    // out stops the agent with everything it started.
    let waited = tokio::select! {
        waited = work => waited,
        () = time_limit_passed(timeout) => {
            return Err(format!(
                "{TIMED_OUT} {} s; the agent was stopped",
                timeout.as_secs()
            ));
        }
        () = stop_now_pressed(stop) => {
            return Err(format!("{STOPPED_BY_USER}; the agent was stopped"));
        }
    };
    match waited {
        Ok((stdout, found, stderr, status)) => Ok((
            Finished {
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
                status,
            },
            found,
        )),
        Err(e) => Err(format!("error while waiting for the agent: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn fake_agent(dir: &Path, script: &str) -> Command {
        Command::new(harness_fake::install(dir, "agent", script))
    }

    /// Sends one line, then reads lines until the agent's answer.
    async fn ask(mut stdin: AgentStdin, mut lines: AgentLines<'_>) -> (AgentLines<'_>, String) {
        stdin.send("ping").await.unwrap();
        let answer = lines.next().await.unwrap().unwrap_or_default();
        (lines, answer)
    }

    #[tokio::test]
    async fn a_conversation_gets_its_answers_and_the_whole_output() {
        let dir = tempfile::tempdir().unwrap();
        let command = fake_agent(
            dir.path(),
            "read-line\nprint pong key-12345678\nprint bye\n",
        );

        let (finished, answer) = run_talking(
            command,
            Duration::from_secs(10),
            &["key-12345678"],
            &StopRequest::new(),
            ask,
        )
        .await
        .unwrap();

        assert_eq!(answer, "pong key-12345678");
        assert!(finished.status.success());
        // The rest the agent printed is not read after the talk; what was
        // read is all there.
        assert!(finished.stdout.starts_with("pong key-12345678\n"));
    }

    #[tokio::test]
    async fn an_agent_that_keeps_running_after_the_talk_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let command = fake_agent(dir.path(), "read-line\nprint pong\nsleep 60\n");
        let started = std::time::Instant::now();

        let (_, answer) = run_talking(
            command,
            Duration::from_secs(50),
            &[],
            &StopRequest::new(),
            ask,
        )
        .await
        .unwrap();

        assert_eq!(answer, "pong");
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[tokio::test]
    async fn the_time_limit_stops_a_conversation_that_never_ends() {
        let dir = tempfile::tempdir().unwrap();
        let command = fake_agent(dir.path(), "sleep 60\n");

        let error = run_talking(
            command,
            Duration::from_secs(1),
            &[],
            &StopRequest::new(),
            ask,
        )
        .await
        .unwrap_err();

        assert!(error.starts_with(TIMED_OUT), "{error}");
    }
}
