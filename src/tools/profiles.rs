//! list_profiles tool, and the input shared by tools that take only a
//! profile.

use crate::config::Profiles;
use crate::providers::Provider;
use crate::types::ToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input for tools whose only argument is the profile to run against.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ProfileInput {
    /// Provider profile to use (see `list_profiles`). Defaults to the
    /// server's default profile.
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    pub model_management: bool,
    pub responses_api: bool,
    pub embeddings: bool,
    pub text_completion: bool,
}

impl Capabilities {
    fn of(provider: Provider) -> Self {
        Self {
            model_management: provider.supports_model_management(),
            responses_api: provider.supports_responses_api(),
            embeddings: provider.supports_embeddings(),
            text_completion: provider.supports_text_completion(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProfileSummary {
    pub name: String,
    pub provider: String,
    pub base_url: String,
    /// Used when a tool is called without a `profile`.
    pub is_default: bool,
    /// Whether an API key is configured. The key itself is never returned.
    pub has_api_key: bool,
    pub capabilities: Capabilities,
}

/// Describe every configured profile. Makes no network calls; use
/// `health_check` with a `profile` to test reachability.
pub fn list_profiles(profiles: &Profiles) -> ToolResult<Vec<ProfileSummary>> {
    let summaries: Vec<ProfileSummary> = profiles
        .profiles
        .iter()
        .map(|p| ProfileSummary {
            name: p.name.clone(),
            provider: p.provider.to_string(),
            base_url: p.base_url.clone(),
            is_default: p.name == profiles.default,
            has_api_key: p.api_key.is_some(),
            capabilities: Capabilities::of(p.provider),
        })
        .collect();
    ToolResult::ok(
        format!(
            "{} profile(s) configured; default is '{}'",
            summaries.len(),
            profiles.default
        ),
        summaries,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_every_profile_without_leaking_keys() {
        let profiles = Profiles::from_toml(
            "default_profile = \"b\"\n[profiles.a]\nprovider = \"ollama\"\n\
             [profiles.b]\nprovider = \"openai\"\napi_key_env = \"K\"",
            |_| Some("sk-secret".to_string()),
        )
        .unwrap();
        let result = list_profiles(&profiles);
        let rendered = serde_json::to_string(&result).unwrap();
        assert!(!rendered.contains("sk-secret"), "{rendered}");
        let data = result.data.unwrap();
        assert_eq!(data.len(), 2);
        assert!(!data[0].is_default && data[1].is_default);
        assert!(!data[0].has_api_key && data[1].has_api_key);
        assert!(data[0].capabilities.model_management);
        assert!(!data[1].capabilities.model_management);
    }
}
