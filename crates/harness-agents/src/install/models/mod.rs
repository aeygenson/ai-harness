//! Asking each agent which models it offers (see `harness_core::models`).
//!
//! Every agent is asked the way it runs a role: with a copy of the saved
//! login in a private temporary folder that is deleted afterwards, and with
//! only the whitelisted environment. A login Codex refreshed while answering
//! is saved back, as after a role. Nothing is sent to a model:
//!
//! - Claude Code: the `initialize` request of its stream-json mode;
//! - Codex: `codex debug models`;
//! - Antigravity: `agy models`;
//! - `DeepSeek` Harness: `GET /models` of `DeepSeek`'s API with the saved key,
//!   with dsh's effort levels; with NVIDIA, `GET /v1/models` of NVIDIA's API.
//!   NVIDIA lists every model it serves, also ones not made for chat.

mod parse;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use harness_core::config::AgentKind;
use harness_core::models::{Model, ModelList};

use crate::adapters::codex::{keep_refreshed_login, AUTH_FILE};
use crate::adapters::dsh::Provider;
use crate::install::credentials;
use crate::process::base_command;
pub use parse::{parse_agy, parse_api_models, parse_claude, parse_codex, with_dsh_efforts};

/// How long one agent may take to answer.
const TIME_LIMIT: Duration = Duration::from_secs(90);

const DEEPSEEK_MODELS_URL: &str = "https://api.deepseek.com/models";
/// NVIDIA's free API (build.nvidia.com), which dsh's `nvidia` route uses too.
const NVIDIA_MODELS_URL: &str = "https://integrate.api.nvidia.com/v1/models";

/// The programs to run; tests put fake ones here.
#[derive(Debug, Clone)]
pub struct Programs {
    /// The Claude Code program.
    pub claude: PathBuf,
    /// The Codex CLI program.
    pub codex: PathBuf,
    /// The `curl` program, which asks `DeepSeek`'s and NVIDIA's APIs for their models.
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
                .map_err(|_missing| "no Claude login saved".to_string())?;
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
            let saved = credentials_dir.join("codex").join(AUTH_FILE);
            let auth = fs::read(&saved).map_err(|_missing| "no Codex login saved".to_string())?;
            harness_platform::private::write(&home.join(AUTH_FILE), &auth)
                .map_err(|e| e.to_string())?;
            let mut command = base_command(&programs.codex, tmp);
            command.env("CODEX_HOME", &home).args(["debug", "models"]);
            let answer = run(command, "", &[]);
            // Asking may have refreshed the login, even when it failed.
            let kept = keep_refreshed_login(&home.join(AUTH_FILE), &auth, &saved).map_err(|e| {
                format!("Codex refreshed its login, but it could not be saved ({e}); sign in to Codex again")
            });
            let answer = answer.map_err(|e| match &kept {
                Err(lost) => format!("{e}; {lost}"),
                Ok(()) => e,
            });
            let list = parse_codex(&answer?)?;
            kept?;
            list
        }
        AgentKind::Dsh => with_dsh_efforts(ask_api(
            Provider::DeepSeek,
            credentials_dir,
            &programs.curl,
            tmp,
        )?),
        // Each NVIDIA model knows its own levels, and the list does not say
        // which, so none are offered; the model's own default is used.
        AgentKind::DshNvidia => ask_api(Provider::Nvidia, credentials_dir, &programs.curl, tmp)?,
        AgentKind::Antigravity => {
            let home = tmp.join("home");
            crate::adapters::antigravity::copy_dir(&credentials_dir.join("antigravity"), &home)
                .map_err(|_missing| "no Antigravity login saved".to_string())?;
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

/// Asks a dsh `provider`'s API for its models with `curl` (`GET /models`).
/// The key goes in a private file in `tmp`, not on the command line.
fn ask_api(
    provider: Provider,
    credentials_dir: &Path,
    curl: &Path,
    tmp: &Path,
) -> Result<Vec<Model>, String> {
    let key = credentials::load_api_key(credentials_dir, provider)
        .map_err(|_missing| format!("no {provider} API key saved"))?;
    let header = tmp.join("header");
    credentials::write_private(
        &header,
        &format!("Authorization: Bearer {}\n", key.expose()),
    )
    .map_err(|e| e.to_string())?;
    let url = match provider {
        Provider::DeepSeek => DEEPSEEK_MODELS_URL,
        Provider::Nvidia => NVIDIA_MODELS_URL,
    };
    let mut command = base_command(curl, tmp);
    command
        .args(["-sS", "-m", "60", "-H"])
        .arg(format!("@{}", header.display()))
        .arg(url);
    parse_api_models(&run(command, "", &[key.expose()])?, provider)
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
    use crate::install::credentials::Secret;

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
        assert_eq!(models[2].efforts, Vec::<String>::new());
        parse_claude("{\"type\":\"result\"}").unwrap_err();
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
    fn nvidia_lists_its_models_without_levels_and_picks_its_default() {
        let models = parse_api_models(
            r#"{"object":"list","data":[{"id":"z-ai/glm-5.3"},{"id":"moonshotai/kimi-k2.6"}]}"#,
            Provider::Nvidia,
        )
        .unwrap();
        assert!(models[1].default && !models[0].default);
        assert!(models.iter().all(|m| m.efforts.is_empty()));
        let error = parse_api_models("<html>", Provider::Nvidia).unwrap_err();
        assert!(error.starts_with("NVIDIA gave no model list"), "{error}");
        let error = parse_api_models(r#"{"status":401,"title":"Unauthorized"}"#, Provider::Nvidia)
            .unwrap_err();
        assert!(error.contains("no `data`"), "{error}");
        // Without the default model in the list, nothing else is marked default.
        let models = parse_api_models(r#"{"data":[{"id":"nv-embed"}]}"#, Provider::Nvidia).unwrap();
        assert!(!models[0].default);
    }

    #[test]
    fn nvidia_is_asked_with_its_own_key_in_a_header_file() {
        let creds = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        credentials::save_token(creds.path(), "nvidia", &Secret::new("nvapi-secret")).unwrap();
        let programs = Programs {
            // `-sS -m 60 -H @<file> <url>`: the key must be in the file, the
            // URL must be NVIDIA's. (`NVIDIA_API_KEY` in the shell running
            // the tests would win over the saved key, so only its place is checked.)
            curl: harness_fake::install(
                bin.path(),
                "curl",
                r#"need-text ${5#@} nvapi- else 4
need-args -sS -m 60 -H ${5} https://integrate.api.nvidia.com/v1/models else 5
print {"data":[{"id":"moonshotai/kimi-k2.6"}]}"#,
            ),
            ..Programs::default()
        };
        let list = ask(AgentKind::DshNvidia, creds.path(), &programs).unwrap();
        assert_eq!(list.agent, AgentKind::DshNvidia);
        assert_eq!(list.models[0].id, "moonshotai/kimi-k2.6");
    }

    #[test]
    fn deepseek_and_antigravity_lists_have_no_levels() {
        let models = parse_api_models(
            r#"{"object":"list","data":[{"id":"deepseek-v4-pro"},{"id":"deepseek-flash"}]}"#,
            Provider::DeepSeek,
        )
        .unwrap();
        assert!(models[1].default && !models[0].default);
        assert!(
            parse_api_models(r#"{"error":{"message":"bad key"}}"#, Provider::DeepSeek)
                .unwrap_err()
                .contains("bad key")
        );

        let dsh = with_dsh_efforts(
            parse_api_models(
                r#"{"data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#,
                Provider::DeepSeek,
            )
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

    #[test]
    fn a_login_codex_refreshed_is_kept_even_when_asking_failed() {
        let creds = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        fs::create_dir_all(creds.path().join("codex")).unwrap();
        fs::write(
            creds.path().join("codex/auth.json"),
            "{\"tokens\":{\"refresh_token\":\"one\"}}",
        )
        .unwrap();
        let programs = Programs {
            codex: harness_fake::install(
                bin.path(),
                "codex",
                "save-text ${CODEX_HOME}/auth.json {\"tokens\":{\"refresh_token\":\"two\"}}\nexit 1",
            ),
            ..Programs::default()
        };

        ask(AgentKind::Codex, creds.path(), &programs).unwrap_err();

        let saved = fs::read_to_string(creds.path().join("codex/auth.json")).unwrap();
        assert_eq!(saved, "{\"tokens\":{\"refresh_token\":\"two\"}}");
    }

    #[test]
    fn an_agent_is_asked_with_a_copy_of_its_login_and_a_failure_hides_the_key() {
        let creds = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        fs::create_dir_all(creds.path().join("codex")).unwrap();
        fs::write(
            creds.path().join("codex/auth.json"),
            "{\"tokens\":{\"refresh_token\":\"one\"}}",
        )
        .unwrap();
        credentials::save_token(creds.path(), "deepseek", &Secret::new("sk-secret")).unwrap();
        let script = |name: &str, script: &str| harness_fake::install(bin.path(), name, script);
        let programs = Programs {
            codex: script(
                "codex",
                r#"need-args debug models else 3
need-text ${CODEX_HOME}/auth.json one else 4
save-text ${CODEX_HOME}/auth.json {"tokens":{"refresh_token":"two"}}
print {"models":[{"slug":"gpt-x","visibility":"list","supported_reasoning_levels":[{"effort":"low"}]}]}"#,
            ),
            curl: script("curl", "eprint-file ${5#@}\nexit 22"),
            ..Programs::default()
        };
        let list = ask(AgentKind::Codex, creds.path(), &programs).unwrap();
        assert_eq!(list.agent, AgentKind::Codex);
        assert_eq!(list.models[0].id, "gpt-x");
        // Codex refreshed its login while answering: the new one is kept.
        let saved = fs::read_to_string(creds.path().join("codex/auth.json")).unwrap();
        assert_eq!(saved, "{\"tokens\":{\"refresh_token\":\"two\"}}");
        let error = ask(AgentKind::Dsh, creds.path(), &programs).unwrap_err();
        assert!(!error.contains("sk-secret"), "{error}");
        assert!(error.contains("***"), "{error}");
        // Only agents with a saved login are asked.
        let asked: Vec<AgentKind> = ask_all(creds.path(), &programs)
            .into_iter()
            .map(|(agent, _)| agent)
            .collect();
        // (An NVIDIA key in the shell running the tests would add dsh+nvidia.)
        assert!(
            asked.starts_with(&[AgentKind::Codex, AgentKind::Dsh]),
            "{asked:?}"
        );
    }
}
