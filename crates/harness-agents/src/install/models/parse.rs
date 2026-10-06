//! Reading each agent's list of models from what it printed.

use harness_core::models::Model;
use serde_json::Value;

use crate::adapters::dsh::DEFAULT_MODEL as DEEPSEEK_DEFAULT_MODEL;

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
    let json: Value = serde_json::from_str(output)
        .map_err(|error| format!("DeepSeek gave no model list: {error}"))?;
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

/// DeepSeek's models as DeepSeek Harness runs them: with its effort levels.
pub fn with_dsh_efforts(models: Vec<Model>) -> Vec<Model> {
    models
        .into_iter()
        .map(|model| Model {
            efforts: crate::adapters::dsh::EFFORTS
                .iter()
                .map(|e| e.to_string())
                .collect(),
            default_effort: Some(crate::adapters::dsh::DEFAULT_EFFORT.to_string()),
            ..model
        })
        .collect()
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
