//! Asking each agent which models it offers (see `harness_core::models`).
//!
//! Every agent is asked the way it runs a role: with a copy of the saved
//! login in a private temporary folder that is deleted afterwards, and with
//! only the whitelisted environment. Nothing is sent to a model:
//!
//! - Claude Code: the `initialize` request of its stream-json mode;
//! - Codex: `codex debug models`;
//! - Antigravity: `agy models`;
//! - DeepSeek Harness: `GET /models` of DeepSeek's API with the saved key,
//!   with dsh's effort levels.

mod parse;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use harness_core::config::AgentKind;
use harness_core::models::ModelList;

use crate::adapters::dsh::KEY_ENV as DEEPSEEK_KEY_ENV;
use crate::install::credentials::{self, Secret};
use crate::process::base_command;
pub use parse::{parse_agy, parse_claude, parse_codex, parse_deepseek, with_dsh_efforts};

/// How long one agent may take to answer.
const TIME_LIMIT: Duration = Duration::from_secs(90);

const DEEPSEEK_MODELS_URL: &str = "https://api.deepseek.com/models";

/// The programs to run; tests put fake ones here.
#[derive(Debug, Clone)]
pub struct Programs {
    /// The Claude Code program.
    pub claude: PathBuf,
    /// The Codex CLI program.
    pub codex: PathBuf,
    /// The `curl` program, which asks DeepSeek's API for its models.
    pub curl: PathBuf,
    /// The Antigravity CLI program.
    pub agy: PathBuf,
}

impl Default for Programs {
    fn default() -> Self {
        Self {
            claude: "claude".into(),
            codex: "codex".into(),
            curl: "curl".into(),
            agy: "agy".into(),
        }
    }
}

/// Asks every agent that has a saved login, all at once. Agents without a
/// login are left out.
pub fn ask_all(
    credentials_dir: &Path,
    programs: &Programs,
) -> Vec<(AgentKind, Result<ModelList, String>)> {
    let handles: Vec<_> = AgentKind::ALL
        .into_iter()
        .filter(|&agent| credentials::has_login(credentials_dir, agent))
        .map(|agent| {
            let (dir, programs) = (credentials_dir.to_path_buf(), programs.clone());
            (agent, thread::spawn(move || ask(agent, &dir, &programs)))
        })
        .collect();
    handles
        .into_iter()
        .map(|(agent, handle)| {
            let result = handle
                .join()
                .unwrap_or_else(|_| Err("asking stopped unexpectedly".into()));
            (agent, result)
        })
        .collect()
}

/// Asks one agent (as harness.toml names it) for its models.
pub fn ask(
    agent: AgentKind,
    credentials_dir: &Path,
    programs: &Programs,
) -> Result<ModelList, String> {
    let dir = tempfile::Builder::new()
        .prefix("harness-models-")
        .tempdir()
        .map_err(|e| format!("cannot make a temporary folder: {e}"))?;
    let tmp = dir.path();
    let models = match agent {
        AgentKind::Claude => {
            let token = credentials::load_token(credentials_dir, "claude")
                .map_err(|_| "no Claude login saved".to_string())?;
            let mut command = base_command(&programs.claude, tmp);
            command
                .env("CLAUDE_CONFIG_DIR", tmp.join("claude"))
                .env("CLAUDE_CODE_OAUTH_TOKEN", token.expose())
                .env("DISABLE_AUTOUPDATER", "1")
                .args(["-p", "--input-format", "stream-json"])
                .args(["--output-format", "stream-json", "--verbose"]);
            let request =
                r#"{"type":"control_request","request_id":"1","request":{"subtype":"initialize"}}"#;
            parse_claude(&run(command, &format!("{request}\n"), &[token.expose()])?)?
        }
        AgentKind::Codex => {
            let home = tmp.join("codex");
            fs::create_dir_all(&home).map_err(|e| e.to_string())?;
            let auth = fs::read_to_string(credentials_dir.join("codex/auth.json"))
                .map_err(|_| "no Codex login saved".to_string())?;
            credentials::write_private(&home.join("auth.json"), &auth)
                .map_err(|e| e.to_string())?;
            let mut command = base_command(&programs.codex, tmp);
            command.env("CODEX_HOME", &home).args(["debug", "models"]);
            parse_codex(&run(command, "", &[])?)?
        }
        AgentKind::Dsh => {
            let key = match std::env::var(DEEPSEEK_KEY_ENV) {
                Ok(key) if !key.trim().is_empty() => Secret::new(key.trim()),
                _ => credentials::load_token(credentials_dir, "deepseek")
                    .map_err(|_| "no DeepSeek API key saved".to_string())?,
            };
            // The key goes in a private file, not on the command line.
            let header = tmp.join("header");
            credentials::write_private(
                &header,
                &format!("Authorization: Bearer {}\n", key.expose()),
            )
            .map_err(|e| e.to_string())?;
            let mut command = base_command(&programs.curl, tmp);
            command
                .args(["-sS", "-m", "60", "-H"])
                .arg(format!("@{}", header.display()))
                .arg(DEEPSEEK_MODELS_URL);
            with_dsh_efforts(parse_deepseek(&run(command, "", &[key.expose()])?)?)
        }
        AgentKind::Antigravity => {
            let home = tmp.join("home");
            crate::adapters::antigravity::copy_dir(&credentials_dir.join("antigravity"), &home)
                .map_err(|_| "no Antigravity login saved".to_string())?;
            let mut command = base_command(&programs.agy, tmp);
            harness_platform::home::set_for(&mut command, &home);
            command
                .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
                .arg("models");
            parse_agy(&run(command, "", &[])?)
        }
    };
    if models.is_empty() {
        return Err("the agent listed no models".into());
    }
    let fetched = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(ModelList {
        agent,
        fetched,
        models,
    })
}

/// Runs `command` with `input`, waits at most `TIME_LIMIT`, returns what it
/// printed. `secrets` never appear in an error.
pub(crate) fn run(command: Command, input: &str, secrets: &[&str]) -> Result<String, String> {
    run_for(command, input, secrets, TIME_LIMIT)
}

/// [`run`] with its own time limit.
pub(crate) fn run_for(
    mut command: Command,
    input: &str,
    secrets: &[&str],
    limit: Duration,
) -> Result<String, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    // An agent asked for its models may start helpers; a time-out stops them too.
    harness_platform::process::own_group(&mut command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child =
        crate::process::spawn(&mut command).map_err(|e| format!("cannot start {program}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = read(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = read(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < limit => thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                harness_platform::process::kill_tree(&mut child);
                return Err(format!("{program} did not answer in {} s", limit.as_secs()));
            }
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    // A helper left running could keep the output open and block the reading.
    harness_platform::process::kill_tree_of(child.id());
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if status.success() {
        return Ok(stdout);
    }
    let why = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let why = harness_core::secret::hide(why, secrets);
    Err(format!("{program} ended with {status}: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claudes_initialize_answer_gives_models_and_levels() {
        let output = concat!(
            "{\"type\":\"system\"}\n",
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"1","response":{"models":["#,
            r#"{"value":"default","resolvedModel":"claude-sonnet-5","displayName":"Default (recommended)","supportedEffortLevels":["low","high"]},"#,
            r#"{"value":"sonnet","resolvedModel":"claude-sonnet-5","displayName":"Sonnet","supportsEffort":true,"supportedEffortLevels":["low","medium","high","max"]},"#,
            r#"{"value":"opus","resolvedModel":"claude-opus-5-5","displayName":"Opus","supportsEffort":true,"supportedEffortLevels":["low","high","xhigh"]},"#,
            r#"{"value":"haiku","resolvedModel":"claude-haiku-4-5","displayName":"Haiku"}"#,
            "]}}}\n"
        );
        let models = parse_claude(output).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["sonnet", "opus", "haiku"]);
        assert!(models[0].default);
        assert_eq!(models[0].name.as_deref(), Some("Sonnet · claude-sonnet-5"));
        assert_eq!(models[1].efforts, ["low", "high", "xhigh"]);
        assert_eq!(models[1].default_effort.as_deref(), Some("high"));
        assert!(models[2].efforts.is_empty());
        assert!(parse_claude("{\"type\":\"result\"}").is_err());
    }

    #[test]
    fn codex_lists_its_visible_models_with_their_levels() {
        let output = r#"{"models":[
            {"slug":"gpt-6.1-sol","display_name":"gpt-6.1-sol","visibility":"list","default_reasoning_level":"low",
             "supported_reasoning_levels":[{"effort":"low"},{"effort":"high"},{"effort":"ultra"}]},
            {"slug":"gpt-reserve","visibility":"hide","default_reasoning_level":"medium"},
            {"slug":"gpt-5.5","display_name":"GPT 5.5","visibility":"list","default_reasoning_level":"medium",
             "supported_reasoning_levels":[{"effort":"medium"},{"effort":"xhigh"}]}]}"#;
        let models = parse_codex(output).unwrap();
        assert_eq!(models.len(), 2);
        assert!(models[0].default && !models[1].default);
        assert_eq!(models[0].efforts, ["low", "high", "ultra"]);
        assert_eq!(models[0].name, None);
        assert_eq!(models[1].name.as_deref(), Some("GPT 5.5"));
        assert_eq!(models[1].default_effort.as_deref(), Some("medium"));
    }

    #[test]
    fn deepseek_and_antigravity_lists_have_no_levels() {
        let models = parse_deepseek(
            r#"{"object":"list","data":[{"id":"deepseek-v4-pro"},{"id":"deepseek-flash"}]}"#,
        )
        .unwrap();
        assert!(models[1].default && !models[0].default);
        assert!(parse_deepseek(r#"{"error":{"message":"bad key"}}"#)
            .unwrap_err()
            .contains("bad key"));

        let dsh = with_dsh_efforts(
            parse_deepseek(r#"{"data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#)
                .unwrap(),
        );
        assert_eq!(dsh[1].efforts, ["off", "low", "high", "max"]);
        assert_eq!(dsh[0].default_effort.as_deref(), Some("high"));
        assert!(dsh[0].default);

        let models = parse_agy(
            "gemini-3.8-flash-high\tGemini 3.8 Flash (High)\nclaude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)\nFetching available models...\n",
        );
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gemini-3.8-flash-high", "claude-opus-4-6-thinking"]);
        assert!(models[0].default && models[0].efforts.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_is_asked_with_a_copy_of_its_login_and_a_failure_hides_the_key() {
        use std::os::unix::fs::PermissionsExt;
        let creds = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        fs::create_dir_all(creds.path().join("codex")).unwrap();
        fs::write(creds.path().join("codex/auth.json"), "{\"login\":1}").unwrap();
        credentials::save_token(creds.path(), "deepseek", &Secret::new("sk-secret")).unwrap();
        let script = |name: &str, body: &str| {
            let path = bin.path().join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let programs = Programs {
            codex: script(
                "codex",
                r#"test "$1 $2" = "debug models" || exit 3
grep -q login "$CODEX_HOME/auth.json" || exit 4
echo '{"models":[{"slug":"gpt-x","visibility":"list","supported_reasoning_levels":[{"effort":"low"}]}]}'"#,
            ),
            curl: script("curl", "cat \"${5#@}\" >&2; exit 22"),
            ..Programs::default()
        };
        let list = ask(AgentKind::Codex, creds.path(), &programs).unwrap();
        assert_eq!(list.agent, AgentKind::Codex);
        assert_eq!(list.models[0].id, "gpt-x");
        let error = ask(AgentKind::Dsh, creds.path(), &programs).unwrap_err();
        assert!(!error.contains("sk-secret"), "{error}");
        assert!(error.contains("***"), "{error}");
        // Only agents with a saved login are asked.
        let asked: Vec<AgentKind> = ask_all(creds.path(), &programs)
            .into_iter()
            .map(|(agent, _)| agent)
            .collect();
        assert_eq!(asked, [AgentKind::Codex, AgentKind::Dsh]);
    }
}
