//! The set of live backend clients, one per configured profile, and the
//! rule for picking one for a tool call.

use crate::client::ApiClient;
use crate::config::Profiles;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ProfileRegistry {
    default: String,
    clients: BTreeMap<String, Arc<ApiClient>>,
}

impl ProfileRegistry {
    pub fn new(profiles: &Profiles) -> Self {
        let clients = profiles
            .profiles
            .iter()
            .map(|p| (p.name.clone(), Arc::new(ApiClient::new(p))))
            .collect();
        Self {
            default: profiles.default.clone(),
            clients,
        }
    }

    /// The client for `profile`, or for the default profile when `None`.
    /// The error is a message naming the available profiles.
    pub fn resolve(&self, profile: Option<&str>) -> Result<Arc<ApiClient>, String> {
        let name = profile.map(str::trim).filter(|n| !n.is_empty());
        let name = name.unwrap_or(&self.default);
        self.clients.get(name).cloned().ok_or_else(|| {
            format!(
                "unknown profile '{name}' (available: {})",
                self.clients.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ProfileRegistry {
        let profiles = Profiles::from_toml(
            "default_profile = \"a\"\n[profiles.a]\nprovider = \"ollama\"\n\
             [profiles.b]\nprovider = \"lmstudio\"",
            |_| None,
        )
        .unwrap();
        ProfileRegistry::new(&profiles)
    }

    #[test]
    fn omitted_or_blank_profile_resolves_to_the_default() {
        let r = registry();
        assert_eq!(r.resolve(None).unwrap().profile(), "a");
        assert_eq!(r.resolve(Some("  ")).unwrap().profile(), "a");
    }

    #[test]
    fn named_profile_resolves_to_its_own_client() {
        let r = registry();
        let b = r.resolve(Some("b")).unwrap();
        assert_eq!(b.profile(), "b");
        assert_eq!(b.base_url(), "http://127.0.0.1:1234");
    }

    #[test]
    fn unknown_profile_lists_the_available_ones() {
        let err = registry().resolve(Some("nope")).err().unwrap();
        assert!(err.contains("nope") && err.contains("a, b"), "{err}");
    }
}
