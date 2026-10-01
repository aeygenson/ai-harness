//! The models each agent offers, and their effort levels.
//!
//! Nothing here is written by hand: `harness models --refresh` (or «Refresh
//! models» in the TUI) asks every agent with a saved login, and the answer is
//! kept in `~/.harness/models/<agent>.json`, so the Roles tab works without
//! the network. The asking itself is in `harness_agents::models`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MODELS_DIR: &str = "models";

/// What one agent said about its models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelList {
    pub agent: String,
    /// When it was asked, in seconds since 1970.
    pub fetched: u64,
    pub models: Vec<Model>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    /// What goes into `model = "..."`.
    pub id: String,
    /// What the agent calls it, if that says more than the id.
    #[serde(default)]
    pub name: Option<String>,
    /// The effort levels this model takes, weakest first; empty if none.
    #[serde(default)]
    pub efforts: Vec<String>,
    /// The level the agent itself uses for this model.
    #[serde(default)]
    pub default_effort: Option<String>,
    /// The agent's own default model.
    #[serde(default)]
    pub default: bool,
}

impl ModelList {
    pub fn find(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }

    /// The model and effort a role gets when it switches to this agent: the
    /// agent's default model, at its default level.
    pub fn default_choice(&self) -> (Option<String>, Option<String>) {
        let Some(model) = self
            .models
            .iter()
            .find(|m| m.default)
            .or_else(|| self.models.first())
        else {
            return (None, None);
        };
        (Some(model.id.clone()), model.default_effort.clone())
    }
}

impl Model {
    /// The level to keep when switching to this model: `effort` if the model
    /// takes it, otherwise its own default.
    pub fn effort_for(&self, effort: Option<&str>) -> Option<String> {
        match effort {
            Some(e) if self.efforts.iter().any(|x| x == e) => Some(e.to_string()),
            _ => self.default_effort.clone(),
        }
    }
}

/// `<home>/models/<agent>.json` (`home` is `~/.harness`).
pub fn cache_path(home: &Path, agent: &str) -> PathBuf {
    home.join(MODELS_DIR).join(format!("{agent}.json"))
}

/// The list saved last time, if any.
pub fn load(home: &Path, agent: &str) -> Option<ModelList> {
    let text = fs::read_to_string(cache_path(home, agent)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(home: &Path, list: &ModelList) -> io::Result<()> {
    let path = cache_path(home, &list.agent);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(list).map_err(io::Error::other)?;
    fs::write(path, text + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, efforts: &[&str], default_effort: Option<&str>, default: bool) -> Model {
        Model {
            id: id.into(),
            name: None,
            efforts: efforts.iter().map(|e| e.to_string()).collect(),
            default_effort: default_effort.map(Into::into),
            default,
        }
    }

    #[test]
    fn lists_are_saved_and_give_the_agents_default() {
        let home = tempfile::tempdir().unwrap();
        let list = ModelList {
            agent: "codex".into(),
            fetched: 1,
            models: vec![
                model(
                    "gpt-6-sol",
                    &["low", "medium", "high"],
                    Some("medium"),
                    false,
                ),
                model("gpt-6.1-sol", &["low", "high"], Some("low"), true),
            ],
        };
        assert_eq!(load(home.path(), "codex"), None);
        save(home.path(), &list).unwrap();
        let loaded = load(home.path(), "codex").unwrap();
        assert_eq!(loaded, list);
        assert_eq!(
            loaded.default_choice(),
            (Some("gpt-6.1-sol".into()), Some("low".into()))
        );
        let sol = loaded.find("gpt-6-sol").unwrap();
        assert_eq!(sol.effort_for(Some("high")).as_deref(), Some("high"));
        assert_eq!(sol.effort_for(Some("ultra")).as_deref(), Some("medium"));
    }
}
