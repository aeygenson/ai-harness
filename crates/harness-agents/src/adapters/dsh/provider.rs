//! Whose API the model of a dsh role runs on: `DeepSeek`'s own, or NVIDIA's
//! free models (build.nvidia.com).
//!
//! dsh reaches NVIDIA through its built-in `nvidia` route (`llm-pi-ai`,
//! OpenAI-compatible, `https://integrate.api.nvidia.com/v1`). The key is
//! looked up by name in dsh's `.credentials.yaml`, exactly like the
//! `DeepSeek` key, so it never goes into the environment.

use std::fmt;

use harness_core::config::AgentKind;
use serde_json::{json, Value};

/// The variable in the user's shell that may hold the NVIDIA API key; it wins
/// over the saved one. dsh's name for the key too.
const NVIDIA_KEY_ENV: &str = "NVIDIA_API_KEY";
/// The model NVIDIA roles use when harness.toml names none: a strong coding
/// model from dsh's NVIDIA catalog.
pub const NVIDIA_DEFAULT_MODEL: &str = "moonshotai/kimi-k2.6";

/// Whose API a dsh role's model runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// `DeepSeek`'s own API (`agent = "dsh"`).
    DeepSeek,
    /// NVIDIA's free API (`agent = "dsh+nvidia"`).
    Nvidia,
}

/// The company whose key it is, as the user knows it: `DeepSeek` or `NVIDIA`.
impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Provider::DeepSeek => "DeepSeek",
            Provider::Nvidia => "NVIDIA",
        })
    }
}

impl Provider {
    /// The agent name in harness.toml that runs dsh with this provider.
    pub fn agent(self) -> AgentKind {
        match self {
            Provider::DeepSeek => AgentKind::Dsh,
            Provider::Nvidia => AgentKind::DshNvidia,
        }
    }

    /// The key's name: in the user's shell, and in dsh's `.credentials.yaml`.
    pub fn key_env(self) -> &'static str {
        match self {
            Provider::DeepSeek => super::KEY_ENV,
            Provider::Nvidia => NVIDIA_KEY_ENV,
        }
    }

    /// The effort levels that work with this provider, weakest first. None
    /// with NVIDIA: with the settings of dsh's catalog, its `pi-ai` layer
    /// sends no level to NVIDIA's API, so a level would silently do nothing.
    pub fn efforts(self) -> &'static [&'static str] {
        match self {
            Provider::DeepSeek => &super::EFFORTS,
            Provider::Nvidia => &[],
        }
    }

    /// The model when harness.toml names none.
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::DeepSeek => super::DEFAULT_MODEL,
            Provider::Nvidia => NVIDIA_DEFAULT_MODEL,
        }
    }

    /// dsh's name for the provider (its "route").
    pub(super) fn route(self) -> &'static str {
        match self {
            Provider::DeepSeek => "deepseek-official",
            Provider::Nvidia => "nvidia",
        }
    }

    /// The patch entries this provider needs in dsh. `DeepSeek`'s route is
    /// always on. For NVIDIA:
    /// - the route is turned on with the key's name and the role's `model`
    ///   (dsh fills in what its catalog knows about the model; a model it
    ///   does not know runs without levels and with dsh's default limits);
    /// - web search is off: dsh searches with `DeepSeek`'s API, and with no
    ///   `DeepSeek` key here it would fail or find one in the project's `.env`
    ///   and send the role's queries to `DeepSeek`.
    pub(super) fn patch_entries(self, model: &str) -> Vec<Value> {
        match self {
            Provider::DeepSeek => Vec::new(),
            Provider::Nvidia => vec![
                json!({
                    "id": "llm-pi-ai",
                    "config": {
                        "providers": {
                            "nvidia": {
                                "apiKeyEnv": NVIDIA_KEY_ENV,
                                "models": [{ "id": model }],
                            },
                        },
                    },
                }),
                json!({ "id": "tool-web", "config": { "search": false } }),
                json!({ "id": "web-search-deepseek", "disabled": true }),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_provider_belongs_to_its_agent_and_key() {
        assert_eq!(Provider::DeepSeek.agent(), AgentKind::Dsh);
        assert_eq!(Provider::Nvidia.agent(), AgentKind::DshNvidia);
        assert_eq!(Provider::DeepSeek.key_env(), "DEEPSEEK_API_KEY");
        assert_eq!(Provider::Nvidia.key_env(), "NVIDIA_API_KEY");
        assert_eq!(Provider::Nvidia.to_string(), "NVIDIA");
    }

    #[test]
    fn only_nvidia_turns_its_route_on_names_the_key_and_stops_web_search() {
        assert_eq!(
            Provider::DeepSeek.patch_entries("deepseek-flash"),
            Vec::<Value>::new()
        );
        let entries = Provider::Nvidia.patch_entries("z-ai/glm-5.3");
        let entry = |id: &str| entries.iter().find(|e| e["id"] == id).unwrap();
        let route = &entry("llm-pi-ai")["config"]["providers"]["nvidia"];
        assert_eq!(route["apiKeyEnv"], "NVIDIA_API_KEY");
        assert_eq!(route["models"][0]["id"], "z-ai/glm-5.3");
        assert_eq!(entry("tool-web")["config"]["search"], false);
        assert_eq!(entry("web-search-deepseek")["disabled"], true);
    }

    #[test]
    fn only_deepseek_takes_levels() {
        assert_eq!(Provider::DeepSeek.efforts(), ["off", "low", "high", "max"]);
        assert_eq!(Provider::Nvidia.efforts(), [] as [&str; 0]);
    }
}
