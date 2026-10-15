//! Whose API the model of a dsh role runs on: `DeepSeek`'s own, or NVIDIA's
//! free models (build.nvidia.com).
//!
//! dsh reaches NVIDIA through its built-in `nvidia` route (`llm-pi-ai`,
//! OpenAI-compatible, `https://integrate.api.nvidia.com/v1`). The key is
//! looked up by name in dsh's `.credentials.yaml`, exactly like the
//! `DeepSeek` key, so it never goes into the environment.

use harness_core::config::AgentKind;
use serde_json::{json, Value};

/// The variable in Lisa's shell that may hold the NVIDIA API key; it wins
/// over the saved one. dsh's name for the key too.
pub const NVIDIA_KEY_ENV: &str = "NVIDIA_API_KEY";
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

impl Provider {
    /// The agent name in harness.toml that runs dsh with this provider.
    pub fn agent(self) -> AgentKind {
        match self {
            Provider::DeepSeek => AgentKind::Dsh,
            Provider::Nvidia => AgentKind::DshNvidia,
        }
    }

    /// The company whose key it is, for messages to Lisa.
    pub fn name(self) -> &'static str {
        match self {
            Provider::DeepSeek => "DeepSeek",
            Provider::Nvidia => "NVIDIA",
        }
    }

    /// The key's name: in Lisa's shell, and in dsh's `.credentials.yaml`.
    pub fn key_env(self) -> &'static str {
        match self {
            Provider::DeepSeek => super::KEY_ENV,
            Provider::Nvidia => NVIDIA_KEY_ENV,
        }
    }

    /// dsh's name for the provider (its "route").
    pub(super) fn route(self) -> &'static str {
        match self {
            Provider::DeepSeek => "deepseek-official",
            Provider::Nvidia => "nvidia",
        }
    }

    /// The model when harness.toml names none.
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::DeepSeek => super::DEFAULT_MODEL,
            Provider::Nvidia => NVIDIA_DEFAULT_MODEL,
        }
    }

    /// The effort when harness.toml names none. NVIDIA's models each know
    /// different levels (some none at all), and dsh refuses a level the
    /// model lacks, so there the model's own default is kept.
    pub(super) fn default_effort(self) -> Option<&'static str> {
        match self {
            Provider::DeepSeek => Some(super::DEFAULT_EFFORT),
            Provider::Nvidia => None,
        }
    }

    /// The patch entry that turns the provider's route on in dsh, if it needs
    /// one. `DeepSeek`'s is always on. NVIDIA's names the key and lists
    /// `model`, so a model newer than dsh's catalog works too (dsh fills in
    /// what it knows about a model it has in its catalog).
    pub(super) fn route_patch(self, model: &str) -> Option<Value> {
        match self {
            Provider::DeepSeek => None,
            Provider::Nvidia => Some(json!({
                "id": "llm-pi-ai",
                "config": {
                    "providers": {
                        "nvidia": {
                            "apiKeyEnv": NVIDIA_KEY_ENV,
                            "models": [{ "id": model }],
                        },
                    },
                },
            })),
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
    }

    #[test]
    fn only_nvidia_turns_its_route_on_and_names_the_key_not_the_value() {
        assert_eq!(Provider::DeepSeek.route_patch("deepseek-flash"), None);
        let patch = Provider::Nvidia.route_patch("z-ai/glm-5.3").unwrap();
        let route = &patch["config"]["providers"]["nvidia"];
        assert_eq!(patch["id"], "llm-pi-ai");
        assert_eq!(route["apiKeyEnv"], "NVIDIA_API_KEY");
        assert_eq!(route["models"][0]["id"], "z-ai/glm-5.3");
    }

    #[test]
    fn nvidia_keeps_the_models_own_effort() {
        assert_eq!(Provider::DeepSeek.default_effort(), Some("high"));
        assert_eq!(Provider::Nvidia.default_effort(), None);
    }
}
