//! Asking each agent which models it offers (see `harness_core::models`).
//!
//! Every agent is asked the way it runs a role: with a copy of the saved
//! login in a private temporary folder that is deleted afterwards, and with
//! only the whitelisted environment. Nothing is sent to a model:
//!
//! - Claude Code: the `initialize` request of its stream-json mode;
//! - Codex: `codex debug models`;
//! - DeepSeek: `GET /models` of its API, with the saved key (Codex itself
//!   only knows OpenAI's models);
//! - Antigravity: `agy models`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use harness_core::config::AGENTS;
use harness_core::models::{Model, ModelList};
use serde_json::Value;

use crate::codex::{DEEPSEEK_DEFAULT_MODEL, DEEPSEEK_KEY_ENV};
use crate::credentials::{self, Secret};

/// How long one agent may take to answer.
const TIME_LIMIT: Duration = Duration::from_secs(90);

const DEEPSEEK_MODELS_URL: &str = "https://api.deepseek.com/models";

/// The programs to run; tests put fake ones here.
#[derive(Debug, Clone)]
pub struct Programs {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub curl: PathBuf,
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
) -> Vec<(String, Result<ModelList, String>)> {
    let handles: Vec<_> = AGENTS
        .iter()
        .filter(|agent| credentials::has_login(credentials_dir, agent))
        .map(|&agent| {
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
            (agent.to_string(), result)
        })
        .collect()
}

/// Asks one agent (as harness.toml names it) for its models.
pub fn ask(agent: &str, credentials_dir: &Path, programs: &Programs) -> Result<ModelList, String> {
    let dir = tempfile::Builder::new()
        .prefix("harness-models-")
        .tempdir()
        .map_err(|e| format!("cannot make a temporary folder: {e}"))?;
    let tmp = dir.path();
    let models = match agent {
        "claude" => {
            let token = credentials::load_token(credentials_dir, "claude")
                .map_err(|_| "no Claude login saved".to_string())?;
            let mut command = command(&programs.claude, tmp);
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
        "codex" => {
            let home = tmp.join("codex");
            fs::create_dir_all(&home).map_err(|e| e.to_string())?;
            let auth = fs::read_to_string(credentials_dir.join("codex/auth.json"))
                .map_err(|_| "no Codex login saved".to_string())?;
            credentials::write_private(&home.join("auth.json"), &auth)
                .map_err(|e| e.to_string())?;
            let mut command = command(&programs.codex, tmp);
            command.env("CODEX_HOME", &home).args(["debug", "models"]);
            parse_codex(&run(command, "", &[])?)?
        }
        "codex+deepseek" => {
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
            let mut command = command(&programs.curl, tmp);
            command
                .args(["-sS", "-m", "60", "-H"])
                .arg(format!("@{}", header.display()))
                .arg(DEEPSEEK_MODELS_URL);
            parse_deepseek(&run(command, "", &[key.expose()])?)?
        }
        "antigravity" => {
            let home = tmp.join("home");
            crate::antigravity::copy_dir(&credentials_dir.join("antigravity"), &home)
                .map_err(|_| "no Antigravity login saved".to_string())?;
            let mut command = command(&programs.agy, tmp);
            harness_platform::home::set_for(&mut command, &home);
            command
                .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
                .arg("models");
            parse_agy(&run(command, "", &[])?)
        }
        other => return Err(format!("unknown agent {other:?}")),
    };
    if models.is_empty() {
        return Err("the agent listed no models".into());
    }
    let fetched = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(ModelList {
        agent: agent.to_string(),
        fetched,
        models,
    })
}

/// `program` in `dir` with only the whitelisted environment.
pub(crate) fn command(program: &Path, dir: &Path) -> Command {
    let mut command = Command::new(harness_platform::program::resolve(program));
    command
        .current_dir(dir)
        .env_clear()
        .envs(harness_platform::env::inherited_values());
    command
}

/// Runs `command` with `input`, waits at most `TIME_LIMIT`, returns what it
/// printed. `secrets` never appear in an error.
pub(crate) fn run(mut command: Command, input: &str, secrets: &[&str]) -> Result<String, String> {
    let program = command.get_program().to_string_lossy().into_owned();
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
            Ok(None) if start.elapsed() < TIME_LIMIT => thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} did not answer in {} s",
                    TIME_LIMIT.as_secs()
                ));
            }
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if status.success() {
        return Ok(stdout);
    }
    let mut why = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        why = why.replace(secret, "***");
    }
    Err(format!("{program} ended with {status}: {why}"))
}

/// Claude Code's answer to `initialize`: `response.models`. The entry
/// `default` only names another model; that one becomes the default.
pub fn parse_claude(output: &str) -> Result<Vec<Model>, String> {
    let response = output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["type"] == "control_response")
        .ok_or("Claude Code gave no answer to the initialize request")?;
    let response = &response["response"];
    let response = if response["response"].is_object() {
        &response["response"]
    } else {
        response
    };
    let entries = response["models"].as_array().cloned().unwrap_or_default();
    let resolved = |entry: &Value| entry["resolvedModel"].as_str().map(str::to_string);
    let default_target = entries
        .iter()
        .find(|e| e["value"] == "default")
        .and_then(resolved);
    let mut models: Vec<Model> = entries
        .iter()
        .filter(|e| e["value"] != "default" && e["disabled"] != true)
        .filter_map(|e| {
            let id = e["value"].as_str()?.to_string();
            let efforts: Vec<String> = if e["supportsEffort"] == false {
                Vec::new()
            } else {
                strings(&e["supportedEffortLevels"])
            };
            let name = match (e["displayName"].as_str(), resolved(e)) {
                (Some(shown), Some(full)) if full != id && shown != id => {
                    Some(format!("{shown} · {full}"))
                }
                (_, Some(full)) if full != id => Some(full),
                (Some(shown), _) if shown != id => Some(shown.to_string()),
                _ => None,
            };
            // Claude names no default level; "high" is its usual one.
            let default_effort = efforts.iter().find(|l| *l == "high").cloned();
            Some(Model {
                id,
                name,
                efforts,
                default_effort,
                default: false,
            })
        })
        .collect();
    // The default is the short name (`sonnet`) of the model `default` points to.
    if let Some(target) = default_target {
        let index = entries
            .iter()
            .filter(|e| e["value"] != "default" && e["disabled"] != true)
            .position(|e| resolved(e).as_deref() == Some(&target) && e["value"] != target)
            .or_else(|| {
                entries
                    .iter()
                    .filter(|e| e["value"] != "default" && e["disabled"] != true)
                    .position(|e| resolved(e).as_deref() == Some(&target))
            });
        if let Some(model) = index.and_then(|i| models.get_mut(i)) {
            model.default = true;
        }
    }
    Ok(models)
}

/// `codex debug models`: the models Codex lists (hidden ones left out), in
/// its own order; the first is its default.
pub fn parse_codex(output: &str) -> Result<Vec<Model>, String> {
    let json: Value =
        serde_json::from_str(output).map_err(|e| format!("Codex printed no model list: {e}"))?;
    let mut models: Vec<Model> = json["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] != "hide")
        .filter_map(|m| {
            Some(Model {
                id: m["slug"].as_str()?.to_string(),
                name: m["display_name"]
                    .as_str()
                    .filter(|n| Some(*n) != m["slug"].as_str())
                    .map(str::to_string),
                efforts: m["supported_reasoning_levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|l| l["effort"].as_str().map(str::to_string))
                    .collect(),
                default_effort: m["default_reasoning_level"].as_str().map(str::to_string),
                default: false,
            })
        })
        .collect();
    if let Some(first) = models.first_mut() {
        first.default = true;
    }
    Ok(models)
}

/// DeepSeek's `GET /models`; no effort levels: reasoning is its own model.
pub fn parse_deepseek(output: &str) -> Result<Vec<Model>, String> {
    let json: Value =
        serde_json::from_str(output).map_err(|_| "DeepSeek gave no model list".to_string())?;
    if let Some(message) = json["error"]["message"].as_str() {
        return Err(format!("DeepSeek: {message}"));
    }
    let ids: Vec<String> = json["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    let default = if ids.iter().any(|id| id == DEEPSEEK_DEFAULT_MODEL) {
        DEEPSEEK_DEFAULT_MODEL.to_string()
    } else {
        ids.first().cloned().unwrap_or_default()
    };
    Ok(ids
        .into_iter()
        .map(|id| Model {
            default: id == default,
            id,
            name: None,
            efforts: Vec::new(),
            default_effort: None,
        })
        .collect())
}

/// `agy models`: `id<TAB>name` per line. The level is part of the id
/// (`gemini-3.8-flash-high`), so there are no separate levels.
pub fn parse_agy(output: &str) -> Vec<Model> {
    let mut models: Vec<Model> = output
        .lines()
        .filter_map(|line| {
            let (id, name) = line.split_once('\t').unwrap_or((line, ""));
            let id = id.trim();
            let ok = !id.is_empty()
                && !id.contains(char::is_whitespace)
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-._/:".contains(c));
            ok.then(|| Model {
                id: id.to_string(),
                name: Some(name.trim().to_string()).filter(|n| !n.is_empty() && n != id),
                efforts: Vec::new(),
                default_effort: None,
                default: false,
            })
        })
        .collect();
    if let Some(first) = models.first_mut() {
        first.default = true;
    }
    models
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
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
        let list = ask("codex", creds.path(), &programs).unwrap();
        assert_eq!(list.agent, "codex");
        assert_eq!(list.models[0].id, "gpt-x");
        let error = ask("codex+deepseek", creds.path(), &programs).unwrap_err();
        assert!(!error.contains("sk-secret"), "{error}");
        assert!(error.contains("***"), "{error}");
        // Only agents with a saved login are asked.
        let asked: Vec<String> = ask_all(creds.path(), &programs)
            .into_iter()
            .map(|(agent, _)| agent)
            .collect();
        assert_eq!(asked, ["codex", "codex+deepseek"]);
    }
}
