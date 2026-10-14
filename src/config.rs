//! Runtime configuration.
//!
//! A server instance holds one or more named *profiles*, each binding a
//! name to one provider, base URL, and API key. Profiles come from a TOML
//! file when one exists (see [`Profiles::load`]); otherwise a single profile
//! named `default` is built from environment variables exactly as it was
//! before profiles existed. Environment variables are read lazily, at
//! connect time, so a client that sets them in its MCP server launch config
//! always wins.

use crate::providers::Provider;
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

/// Name of the profile built from environment variables when no profiles
/// file exists.
pub const ENV_PROFILE_NAME: &str = "default";

/// One resolved profile: everything needed to talk to one backend.
#[derive(Debug, Clone)]
pub struct Config {
    pub name: String,
    pub provider: Provider,
    /// `scheme://host:port`, no trailing slash, no `/v1` or `/api/...` suffix.
    pub base_url: String,
    pub api_key: Option<String>,
}

/// Every configured profile plus which one tools use when called without a
/// `profile` argument.
#[derive(Debug, Clone)]
pub struct Profiles {
    pub default: String,
    pub profiles: Vec<Config>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "{provider} requires an API key — set LLM_API_KEY (or LMSTUDIO_API_TOKEN for lmstudio)"
    )]
    MissingApiKey { provider: Provider },
    #[error("profile '{profile}': {reason}")]
    InvalidProfile { profile: String, reason: String },
    /// A profile needs an API key that is not available. Unlike other
    /// profile errors this is not fatal when loading a profiles file: the
    /// profile is skipped so the others stay usable.
    #[error("profile '{profile}': {reason}")]
    ProfileKeyUnavailable { profile: String, reason: String },
    #[error("profiles file {path}: {reason}")]
    ProfilesFile { path: String, reason: String },
}

impl Config {
    /// Build the single `default` profile from the environment.
    ///
    /// - `LLM_PROVIDER`: one of `lmstudio` (default), `ollama`, `openai`,
    ///   `anthropic`, `openai-compatible`.
    /// - `LLM_BASE_URL`: full `scheme://host:port` override. Falls back to
    ///   provider-specific defaults (and, for `lmstudio` only, the legacy
    ///   `LMSTUDIO_BASE_URL`/`LMSTUDIO_HOST`/`LMSTUDIO_PORT` variables, kept
    ///   working exactly as before for existing configs). Required for
    ///   `openai-compatible`.
    /// - `LLM_API_KEY`: bearer/API key, if the provider needs one. Falls
    ///   back to `LMSTUDIO_API_TOKEN` for `lmstudio`. Required (hard error)
    ///   for `openai` and `anthropic`.
    pub fn from_env() -> Result<Self, ConfigError> {
        let provider = match env::var("LLM_PROVIDER") {
            Ok(raw) => Provider::parse(&raw).unwrap_or_else(|| {
                tracing::warn!("Unknown LLM_PROVIDER '{raw}', falling back to lmstudio");
                Provider::LmStudio
            }),
            Err(_) => Provider::LmStudio,
        };

        let base_url = env::var("LLM_BASE_URL")
            .ok()
            .or_else(|| {
                (provider == Provider::LmStudio)
                    .then(legacy_lmstudio_base_url)
                    .flatten()
            })
            .map(|s| s.trim_end_matches('/').to_string())
            .or_else(|| provider.default_base_url().map(str::to_string))
            .ok_or_else(|| ConfigError::InvalidProfile {
                profile: ENV_PROFILE_NAME.to_string(),
                reason: format!("{provider} has no default URL — set LLM_BASE_URL"),
            })?;

        let api_key = env::var("LLM_API_KEY")
            .ok()
            .or_else(|| {
                (provider == Provider::LmStudio)
                    .then(|| env::var("LMSTUDIO_API_TOKEN").ok())
                    .flatten()
            })
            .filter(|s| !s.is_empty());

        if provider.requires_api_key() && api_key.is_none() {
            return Err(ConfigError::MissingApiKey { provider });
        }

        Ok(Self {
            name: ENV_PROFILE_NAME.to_string(),
            provider,
            base_url,
            api_key,
        })
    }
}

impl Profiles {
    /// Load profiles from the profiles file if there is one, else from the
    /// environment.
    ///
    /// The file is `LLM_PROFILES_FILE` when set (it must exist), otherwise
    /// `<config dir>/lmstudio-rs-mcp/profiles.toml` when present. When a file
    /// is used, the `LLM_*` and `LMSTUDIO_*` provider variables are not
    /// consulted; API keys come from the environment variable each profile
    /// names in `api_key_env`.
    pub fn load() -> Result<Self, ConfigError> {
        match profiles_file_path()? {
            Some(path) => {
                let text =
                    std::fs::read_to_string(&path).map_err(|e| ConfigError::ProfilesFile {
                        path: path.display().to_string(),
                        reason: format!("could not read: {e}"),
                    })?;
                tracing::info!(path = %path.display(), "Loading provider profiles");
                Self::from_toml(&text, |key| env::var(key).ok()).map_err(|e| match e {
                    ConfigError::ProfilesFile { reason, .. } => ConfigError::ProfilesFile {
                        path: path.display().to_string(),
                        reason,
                    },
                    other => other,
                })
            }
            None => {
                let config = Config::from_env()?;
                Ok(Self {
                    default: config.name.clone(),
                    profiles: vec![config],
                })
            }
        }
    }

    /// Parse the profiles file format. `lookup` resolves `api_key_env`
    /// names (injected so tests don't touch the process environment).
    pub fn from_toml(
        text: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let file: ProfilesFile = toml::from_str(text).map_err(|e| ConfigError::ProfilesFile {
            path: String::new(),
            reason: e.to_string(),
        })?;

        let mut profiles = Vec::with_capacity(file.profiles.len());
        let mut skipped: Vec<String> = Vec::new();
        for (name, entry) in file.profiles {
            match entry.resolve(name, &lookup) {
                Ok(profile) => profiles.push(profile),
                Err(ConfigError::ProfileKeyUnavailable { profile, reason }) => {
                    tracing::warn!("Skipping profile '{profile}': {reason}");
                    skipped.push(profile);
                }
                Err(e) => return Err(e),
            }
        }
        let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();

        let default = match file.default_profile {
            Some(wanted) if names.contains(&wanted.as_str()) => wanted,
            Some(wanted) if skipped.contains(&wanted) => {
                return Err(ConfigError::ProfilesFile {
                    path: String::new(),
                    reason: format!(
                        "default_profile '{wanted}' was skipped because its API key is unavailable"
                    ),
                })
            }
            Some(wanted) => {
                return Err(ConfigError::ProfilesFile {
                    path: String::new(),
                    reason: format!(
                        "default_profile '{wanted}' is not defined (profiles: {})",
                        names.join(", ")
                    ),
                })
            }
            None if names.contains(&ENV_PROFILE_NAME) => ENV_PROFILE_NAME.to_string(),
            None if names.len() == 1 => names[0].to_string(),
            None => {
                return Err(ConfigError::ProfilesFile {
                    path: String::new(),
                    reason: if names.is_empty() && !skipped.is_empty() {
                        format!(
                            "every profile was skipped for a missing API key ({})",
                            skipped.join(", ")
                        )
                    } else if names.is_empty() {
                        "no profiles defined — add a [profiles.<name>] table".to_string()
                    } else {
                        format!(
                            "several profiles ({}) and no default — set default_profile",
                            names.join(", ")
                        )
                    },
                })
            }
        };

        Ok(Self { default, profiles })
    }

    #[cfg(test)]
    pub fn get(&self, name: &str) -> Option<&Config> {
        self.profiles.iter().find(|p| p.name == name)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfilesFile {
    default_profile: Option<String>,
    #[serde(default)]
    profiles: BTreeMap<String, ProfileEntry>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileEntry {
    provider: String,
    base_url: Option<String>,
    /// Name of the environment variable holding the API key. The key itself
    /// is deliberately not accepted in the file.
    api_key_env: Option<String>,
}

impl ProfileEntry {
    fn resolve(
        self,
        name: String,
        lookup: &impl Fn(&str) -> Option<String>,
    ) -> Result<Config, ConfigError> {
        let invalid = |reason: String| ConfigError::InvalidProfile {
            profile: name.clone(),
            reason,
        };
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(invalid(
                "name must be non-empty and use only letters, digits, '-', '_' or '.'".to_string(),
            ));
        }
        let provider = Provider::parse(&self.provider).ok_or_else(|| {
            invalid(format!(
                "unknown provider '{}' (expected lmstudio, ollama, openai, anthropic, or openai-compatible)",
                self.provider
            ))
        })?;
        let base_url = self
            .base_url
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .or_else(|| provider.default_base_url().map(str::to_string))
            .ok_or_else(|| invalid(format!("{provider} has no default URL — set base_url")))?;
        match url::Url::parse(&base_url) {
            Ok(u) if matches!(u.scheme(), "http" | "https") && u.host().is_some() => {}
            _ => {
                return Err(invalid(format!(
                    "base_url '{base_url}' must be an http:// or https:// URL"
                )))
            }
        }
        let api_key = match self.api_key_env {
            Some(var) => {
                let key = lookup(&var).filter(|k| !k.is_empty());
                if key.is_none() {
                    return Err(ConfigError::ProfileKeyUnavailable {
                        profile: name,
                        reason: format!("api_key_env names '{var}', which is not set"),
                    });
                }
                key
            }
            None => None,
        };
        if provider.requires_api_key() && api_key.is_none() {
            return Err(ConfigError::ProfileKeyUnavailable {
                profile: name,
                reason: format!("{provider} requires an API key — set api_key_env"),
            });
        }
        Ok(Config {
            name,
            provider,
            base_url,
            api_key,
        })
    }
}

/// Where the profiles file lives, if it should be used.
fn profiles_file_path() -> Result<Option<PathBuf>, ConfigError> {
    if let Ok(explicit) = env::var("LLM_PROFILES_FILE") {
        let path = PathBuf::from(explicit);
        return if path.is_file() {
            Ok(Some(path))
        } else {
            Err(ConfigError::ProfilesFile {
                path: path.display().to_string(),
                reason: "LLM_PROFILES_FILE does not point to a file".to_string(),
            })
        };
    }
    Ok(default_profiles_path().filter(|p| Path::is_file(p)))
}

fn default_profiles_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lmstudio-rs-mcp").join("profiles.toml"))
}

/// Reproduces the pre-multi-provider resolution exactly, for `lmstudio`
/// only: `LMSTUDIO_BASE_URL` directly, else `LMSTUDIO_HOST`/`LMSTUDIO_PORT`
/// (defaults `127.0.0.1`/`1234`) composed into a URL. Returns `None` when
/// none of these are set, so the caller falls through to the provider
/// default.
fn legacy_lmstudio_base_url() -> Option<String> {
    if let Ok(base) = env::var("LMSTUDIO_BASE_URL") {
        return Some(base);
    }
    let host_set = env::var("LMSTUDIO_HOST").is_ok();
    let port_set = env::var("LMSTUDIO_PORT").is_ok();
    if !host_set && !port_set {
        return None;
    }
    let host = env::var("LMSTUDIO_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("LMSTUDIO_PORT").unwrap_or_else(|_| "1234".to_string());
    let port: u16 = port.parse().unwrap_or_else(|_| {
        tracing::warn!("Invalid LMSTUDIO_PORT '{port}', falling back to 1234");
        1234
    });
    Some(format!("http://{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Environment variables are process-global, so these tests serialize
    // against each other with a lock rather than relying on cargo test's
    // default parallelism to not interleave them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const VARS: &[&str] = &[
        "LLM_PROVIDER",
        "LLM_BASE_URL",
        "LLM_API_KEY",
        "LMSTUDIO_BASE_URL",
        "LMSTUDIO_HOST",
        "LMSTUDIO_PORT",
        "LMSTUDIO_API_TOKEN",
        "LLM_PROFILES_FILE",
    ];

    fn with_clean_env(f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved: Vec<(&str, Option<String>)> =
            VARS.iter().map(|v| (*v, env::var(v).ok())).collect();
        for v in VARS {
            unsafe { env::remove_var(v) };
        }
        f();
        for (k, v) in saved {
            match v {
                Some(v) => unsafe { env::set_var(k, v) },
                None => unsafe { env::remove_var(k) },
            }
        }
    }

    #[test]
    fn defaults_to_lmstudio_with_no_env_set() {
        with_clean_env(|| {
            let config = Config::from_env().unwrap();
            assert_eq!(config.provider, Provider::LmStudio);
            assert_eq!(config.base_url, "http://127.0.0.1:1234");
            assert_eq!(config.api_key, None);
        });
    }

    #[test]
    fn legacy_lmstudio_host_and_port_still_work() {
        with_clean_env(|| {
            unsafe {
                env::set_var("LMSTUDIO_HOST", "192.168.1.50");
                env::set_var("LMSTUDIO_PORT", "5678");
            }
            let config = Config::from_env().unwrap();
            assert_eq!(config.base_url, "http://192.168.1.50:5678");
        });
    }

    #[test]
    fn legacy_lmstudio_base_url_takes_precedence_over_host_port() {
        with_clean_env(|| {
            unsafe {
                env::set_var("LMSTUDIO_BASE_URL", "http://legacy:9999");
                env::set_var("LMSTUDIO_HOST", "ignored");
            }
            let config = Config::from_env().unwrap();
            assert_eq!(config.base_url, "http://legacy:9999");
        });
    }

    #[test]
    fn legacy_lmstudio_api_token_still_works() {
        with_clean_env(|| {
            unsafe { env::set_var("LMSTUDIO_API_TOKEN", "secret") };
            let config = Config::from_env().unwrap();
            assert_eq!(config.api_key, Some("secret".to_string()));
        });
    }

    #[test]
    fn new_generic_vars_select_ollama_with_its_own_default_port() {
        with_clean_env(|| {
            unsafe { env::set_var("LLM_PROVIDER", "ollama") };
            let config = Config::from_env().unwrap();
            assert_eq!(config.provider, Provider::Ollama);
            assert_eq!(config.base_url, "http://127.0.0.1:11434");
        });
    }

    #[test]
    fn openai_without_an_api_key_is_a_hard_error() {
        with_clean_env(|| {
            unsafe { env::set_var("LLM_PROVIDER", "openai") };
            assert!(matches!(
                Config::from_env(),
                Err(ConfigError::MissingApiKey {
                    provider: Provider::OpenAi
                })
            ));
        });
    }

    #[test]
    fn anthropic_with_llm_api_key_resolves_cleanly() {
        with_clean_env(|| {
            unsafe {
                env::set_var("LLM_PROVIDER", "anthropic");
                env::set_var("LLM_API_KEY", "sk-ant-test");
            }
            let config = Config::from_env().unwrap();
            assert_eq!(config.provider, Provider::Anthropic);
            assert_eq!(config.base_url, "https://api.anthropic.com");
            assert_eq!(config.api_key, Some("sk-ant-test".to_string()));
        });
    }

    #[test]
    fn llm_base_url_overrides_the_provider_default() {
        with_clean_env(|| {
            unsafe {
                env::set_var("LLM_PROVIDER", "ollama");
                env::set_var("LLM_BASE_URL", "http://remote-box:11434/");
            }
            let config = Config::from_env().unwrap();
            // Trailing slash stripped, same as the legacy behavior.
            assert_eq!(config.base_url, "http://remote-box:11434");
        });
    }

    #[test]
    fn unknown_provider_name_falls_back_to_lmstudio() {
        with_clean_env(|| {
            unsafe { env::set_var("LLM_PROVIDER", "bedrock") };
            let config = Config::from_env().unwrap();
            assert_eq!(config.provider, Provider::LmStudio);
        });
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    const MULTI: &str = r#"
        default_profile = "lmstudio"

        [profiles.lmstudio]
        provider = "lmstudio"

        [profiles.ollama]
        provider = "ollama"
        base_url = "http://box:11434/"

        [profiles.omlx]
        provider = "openai-compatible"
        base_url = "http://127.0.0.1:8000"

        [profiles.claude]
        provider = "anthropic"
        api_key_env = "TEST_ANTHROPIC_KEY"
    "#;

    #[test]
    fn profiles_file_defines_several_backends_at_once() {
        let profiles = Profiles::from_toml(MULTI, |k| {
            (k == "TEST_ANTHROPIC_KEY").then(|| "sk-ant".to_string())
        })
        .unwrap();
        assert_eq!(profiles.default, "lmstudio");
        assert_eq!(profiles.profiles.len(), 4);
        let ollama = profiles.get("ollama").unwrap();
        assert_eq!(ollama.provider, Provider::Ollama);
        assert_eq!(ollama.base_url, "http://box:11434");
        let omlx = profiles.get("omlx").unwrap();
        assert_eq!(omlx.provider, Provider::OpenAiCompatible);
        assert_eq!(omlx.api_key, None);
        assert_eq!(
            profiles.get("claude").unwrap().api_key.as_deref(),
            Some("sk-ant")
        );
        assert_eq!(
            profiles.get("lmstudio").unwrap().base_url,
            "http://127.0.0.1:1234"
        );
    }

    #[test]
    fn default_profile_falls_back_to_one_named_default_then_the_only_one() {
        let named =
            "[profiles.default]\nprovider = \"ollama\"\n[profiles.b]\nprovider = \"lmstudio\"";
        assert_eq!(
            Profiles::from_toml(named, no_env).unwrap().default,
            "default"
        );
        let single = "[profiles.only]\nprovider = \"ollama\"";
        assert_eq!(Profiles::from_toml(single, no_env).unwrap().default, "only");
    }

    #[test]
    fn ambiguous_or_missing_default_is_an_error() {
        let two = "[profiles.a]\nprovider = \"ollama\"\n[profiles.b]\nprovider = \"lmstudio\"";
        let err = Profiles::from_toml(two, no_env).unwrap_err().to_string();
        assert!(err.contains("default_profile"), "{err}");
        let err = Profiles::from_toml("", no_env).unwrap_err().to_string();
        assert!(err.contains("no profiles"), "{err}");
        let err = Profiles::from_toml(
            "default_profile = \"zzz\"\n[profiles.a]\nprovider = \"ollama\"",
            no_env,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("zzz"), "{err}");
    }

    #[test]
    fn invalid_profiles_are_rejected_with_the_profile_name() {
        let cases = [
            ("provider = \"bedrock\"", "unknown provider"),
            ("provider = \"openai-compatible\"", "no default URL"),
            ("provider = \"omlx\"", "no default URL"),
            (
                "provider = \"ollama\"\nbase_url = \"localhost:11434\"",
                "http",
            ),
        ];
        for (body, expect) in cases {
            let err = Profiles::from_toml(&format!("[profiles.p]\n{body}"), no_env)
                .unwrap_err()
                .to_string();
            assert!(err.contains("'p'") && err.contains(expect), "{body}: {err}");
        }
    }

    #[test]
    fn profile_with_an_unavailable_key_is_skipped_not_fatal() {
        let toml = "default_profile = \"local\"\n\
            [profiles.local]\nprovider = \"ollama\"\n\
            [profiles.cloud]\nprovider = \"openai\"\n\
            [profiles.cloud2]\nprovider = \"anthropic\"\napi_key_env = \"UNSET_VAR\"";
        let profiles = Profiles::from_toml(toml, no_env).unwrap();
        assert_eq!(profiles.profiles.len(), 1);
        assert_eq!(profiles.default, "local");
        assert!(profiles.get("cloud").is_none() && profiles.get("cloud2").is_none());
    }

    #[test]
    fn skipped_default_or_all_skipped_is_still_an_error() {
        let skipped_default = "default_profile = \"cloud\"\n\
            [profiles.local]\nprovider = \"ollama\"\n\
            [profiles.cloud]\nprovider = \"openai\"";
        let err = Profiles::from_toml(skipped_default, no_env)
            .unwrap_err()
            .to_string();
        assert!(err.contains("cloud") && err.contains("skipped"), "{err}");
        let err = Profiles::from_toml("[profiles.c]\nprovider = \"openai\"", no_env)
            .unwrap_err()
            .to_string();
        assert!(err.contains("every profile was skipped"), "{err}");
    }

    #[test]
    fn profile_names_and_keys_are_validated() {
        let err = Profiles::from_toml("[profiles.\"bad name\"]\nprovider = \"ollama\"", no_env)
            .unwrap_err()
            .to_string();
        assert!(err.contains("name must"), "{err}");
        // A literal API key in the file is refused, not silently accepted.
        let err = Profiles::from_toml(
            "[profiles.p]\nprovider = \"openai\"\napi_key = \"sk-x\"",
            no_env,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("api_key"), "{err}");
    }

    #[test]
    fn openai_compatible_from_env_requires_a_base_url() {
        with_clean_env(|| {
            unsafe { env::set_var("LLM_PROVIDER", "openai-compatible") };
            assert!(matches!(
                Config::from_env(),
                Err(ConfigError::InvalidProfile { .. })
            ));
            unsafe { env::set_var("LLM_BASE_URL", "http://127.0.0.1:8000") };
            assert_eq!(
                Config::from_env().unwrap().provider,
                Provider::OpenAiCompatible
            );
        });
    }
}
