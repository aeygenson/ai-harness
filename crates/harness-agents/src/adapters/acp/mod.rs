//! Runs a role with an agent that speaks the Agent Client Protocol (ACP):
//! Gemini CLI (`gemini --acp`), or any other agent whose command the user
//! gives (`[acp]` in `harness.toml`).
//!
//! The harness is the protocol's "client": it starts the agent, says hello
//! (`initialize`), opens a session in the project folder with the role's
//! MCP servers (`session/new`), sends the prompt (`session/prompt`) and reads
//! until the turn ends. While the agent works it may ask whether it may run
//! a tool (`session/request_permission`); the answer follows the role's
//! rights (see [`permission`]). The harness offers no file or terminal
//! methods: the agent uses its own tools.
//!
//! The protocol has no way to force an agent to ask first, so, as with
//! every agent, the git check after the role is what decides which changed
//! files stand, and the tripwire watches what git cannot see.

mod generic;
mod permission;
mod protocol;

use std::process::Command;

use harness_core::config::AgentKind;
use harness_core::mcp::McpServer;
use harness_core::secret;
use harness_core::task::agent::{AgentOutcome, RoleJob, RunEnd};
use serde_json::Value;

use crate::process::{self, failed, AgentLines, AgentStdin};
use crate::role_settings::RoleSettings;
pub use generic::Acp;
use permission::Place;
use protocol::{Message, INITIALIZE_ID, NEW_SESSION_ID, PROMPT_ID, PROTOCOL_VERSION};

/// The only end of a turn that means the agent finished its work.
const END_TURN: &str = "end_turn";

/// How the conversation ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Turn {
    /// The agent answered the prompt; the protocol's stop reason, such as `end_turn`.
    Ended(String),
    /// The agent would not open a session, usually for lack of a login.
    NoSession(String),
    /// Something else went wrong before the turn ended; why, for the user.
    Broken(String),
}

/// What [`run_role`] needs besides the job: which agent it is (for the log
/// and the messages) and how to sign in when the agent says it is not.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Speaker<'a> {
    /// The agent, as `harness.toml` names it.
    pub kind: AgentKind,
    /// What to do when the agent refuses for lack of a login.
    pub sign_in: &'a str,
}

/// Runs `command` (the agent, ready to speak ACP on its standard input and
/// output) for `job`, and turns the conversation into the role's outcome.
pub(crate) async fn run_role(
    speaker: Speaker<'_>,
    command: Command,
    job: &RoleJob,
    settings: &RoleSettings,
) -> AgentOutcome {
    let log = settings.header(speaker.kind, job);
    let secrets = settings.server_secrets(job.role);
    let place = Place {
        role: job.role,
        project_dir: &job.project_dir,
        output_dir: &job.output_dir,
    };
    let servers = settings.servers(job.role);
    let talked = process::run_talking(
        command,
        settings.timeout(),
        &secrets,
        &job.stop,
        |stdin, lines| talk(stdin, lines, place, servers, &job.prompt),
    )
    .await;
    let (finished, found) = match talked {
        Ok((finished, found)) => (Ok(finished), Some(found)),
        Err(message) => (Err(message), None),
    };
    // What the agent printed becomes `agent.log`, which is committed: an MCP
    // tool may echo a key back, so the role's secrets are hidden first.
    let finished = match process::hide_secrets(finished, secrets.iter().copied()) {
        Ok(finished) => finished,
        Err(message) => return failed(log, message),
    };
    let Some((turn, notes)) = found else {
        return failed(log, "the agent ended without a conversation".into());
    };
    let mut result = outcome(speaker, log, &finished, turn, &notes);
    // The notes (a refused tool's title) and the agent's error text come from
    // the agent too, so they get the same hiding.
    result.log = secret::hide(&result.log, &secrets);
    if let RunEnd::Failed(message) | RunEnd::TimedOut(message) = &mut result.end {
        *message = secret::hide(message, &secrets);
    }
    result
}

/// Holds the whole conversation and gives the agent's output back with how
/// the turn ended and the harness's notes (the tools it refused).
async fn talk<'s>(
    mut stdin: AgentStdin,
    mut lines: AgentLines<'s>,
    place: Place<'_>,
    servers: &[McpServer],
    prompt: &str,
) -> (AgentLines<'s>, (Turn, Vec<String>)) {
    let mut notes = Vec::new();
    let mut chat = Chat {
        stdin: &mut stdin,
        lines: &mut lines,
        place,
        notes: &mut notes,
    };
    let turn = chat.whole(servers, prompt).await;
    (lines, (turn, notes))
}

/// The harness's side of one conversation.
struct Chat<'c, 's, 'p> {
    stdin: &'c mut AgentStdin,
    lines: &'c mut AgentLines<'s>,
    place: Place<'p>,
    notes: &'c mut Vec<String>,
}

impl Chat<'_, '_, '_> {
    /// Hello, a new session, the prompt; ends with the stop reason.
    async fn whole(&mut self, servers: &[McpServer], prompt: &str) -> Turn {
        let hello = match self.ask(INITIALIZE_ID, &protocol::initialize()).await {
            Ok(hello) => hello,
            Err(turn) => return turn,
        };
        let version = hello.get("protocolVersion").and_then(Value::as_u64);
        if version != Some(PROTOCOL_VERSION) {
            return Turn::Broken(format!(
                "the agent speaks ACP version {}, the harness only version {PROTOCOL_VERSION}",
                version.map_or_else(|| "?".to_string(), |v| v.to_string())
            ));
        }
        let new_session = protocol::new_session(self.place.project_dir, servers);
        let session = match self.ask(NEW_SESSION_ID, &new_session).await {
            Ok(session) => session,
            Err(Turn::Broken(why)) => return Turn::NoSession(why),
            Err(turn) => return turn,
        };
        let Some(session_id) = session.get("sessionId").and_then(Value::as_str) else {
            return Turn::Broken("the agent opened a session without an id".into());
        };
        let answer = match self
            .ask(PROMPT_ID, &protocol::prompt(session_id, prompt))
            .await
        {
            Ok(answer) => answer,
            Err(turn) => return turn,
        };
        let reason = answer.get("stopReason").and_then(Value::as_str);
        Turn::Ended(reason.unwrap_or("none given").to_string())
    }

    /// Sends the request `line` and reads until its answer, meanwhile
    /// answering the agent's own requests.
    async fn ask(&mut self, id: u64, line: &str) -> Result<Value, Turn> {
        self.send(line).await?;
        loop {
            let next = self.lines.next().await;
            let line = match next {
                Ok(Some(line)) => line,
                Ok(None) => return Err(Turn::Broken("the agent ended before it answered".into())),
                Err(e) => return Err(Turn::Broken(format!("cannot read the agent: {e}"))),
            };
            match protocol::read(&line) {
                Message::Answer { id: got, result } if got == id => {
                    return result.map_err(Turn::Broken);
                }
                Message::Request { id, method, params } => {
                    let reply = self.reply(&id, &method, &params);
                    self.send(&reply).await?;
                }
                Message::Answer { .. } | Message::Other => {}
            }
        }
    }

    /// The answer to a request of the agent: permission follows the role,
    /// everything else (files, terminals) is not offered.
    fn reply(&mut self, id: &Value, method: &str, params: &Value) -> String {
        if method != "session/request_permission" {
            return protocol::not_offered(id, method);
        }
        let (result, note) = permission::answer(self.place, params);
        self.notes.extend(note);
        protocol::answer(id, &result)
    }

    async fn send(&mut self, line: &str) -> Result<(), Turn> {
        self.stdin
            .send(line)
            .await
            .map_err(|e| Turn::Broken(format!("cannot write to the agent: {e}")))
    }
}

/// Turns the finished conversation into the role's outcome.
fn outcome(
    speaker: Speaker,
    mut log: String,
    finished: &process::Finished,
    turn: Turn,
    notes: &[String],
) -> AgentOutcome {
    process::append_output(&mut log, finished);
    for note in notes {
        log.push_str(note);
        log.push('\n');
    }
    let agent = speaker.kind;
    let (done, message) = match turn {
        Turn::Ended(reason) if reason == END_TURN => (true, String::new()),
        Turn::Ended(reason) => (false, format!("{agent} stopped before the end: {reason}")),
        Turn::NoSession(why) => (
            false,
            // Usually a missing login, but a bad MCP server entry looks the same.
            format!(
                "{agent} did not start working: {why}. If it is about signing in: {}; \
                 otherwise check the role's MCP servers",
                speaker.sign_in
            ),
        ),
        Turn::Broken(why) => (false, format!("{agent}: {why}")),
    };
    let end = process::run_end(done, message, &finished.stderr);
    if let RunEnd::Failed(message) | RunEnd::TimedOut(message) = &end {
        log.push_str(message);
        log.push('\n');
    }
    AgentOutcome { end, log }
}

#[cfg(test)]
mod tests;
