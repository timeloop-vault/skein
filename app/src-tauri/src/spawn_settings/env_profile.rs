//! Switchable environment profiles (#615): a named set of variables to
//! set or unset, layered on top of the global `extra_env`.
//!
//! Which profile applies to a spawn is decided by
//! [`SpawnSettings::resolve_env_profile`]: a per-harness override beats the
//! one global active profile.

use serde::{Deserialize, Serialize};

use super::SpawnSettings;

/// One variable in a profile: set to `value`, or removed from the child's
/// environment when `unset` (even if it came from the inherited env or the
/// login-shell overlay).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileVar {
    pub key: String,
    pub value: String,
    pub unset: bool,
}

/// A named bundle of [`ProfileVar`]s.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EnvProfile {
    pub name: String,
    pub vars: Vec<ProfileVar>,
}

impl SpawnSettings {
    /// The profile a spawn should apply: the harness override if it names
    /// an existing profile, else the active profile if it exists, else
    /// `None`. Names match trimmed and case-sensitively; with duplicate
    /// names the first wins.
    pub(crate) fn resolve_env_profile(
        &self,
        harness_override: Option<&str>,
    ) -> Option<&EnvProfile> {
        let find = |name: &str| {
            let name = name.trim();
            self.env_profiles.iter().find(|p| p.name.trim() == name)
        };
        harness_override
            .and_then(find)
            .or_else(|| self.active_env_profile.as_deref().and_then(find))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, value: &str) -> EnvProfile {
        EnvProfile {
            name: name.to_owned(),
            vars: vec![ProfileVar {
                key: "K".to_owned(),
                value: value.to_owned(),
                unset: false,
            }],
        }
    }

    fn name_of(p: Option<&EnvProfile>) -> Option<&str> {
        p.map(|p| p.name.as_str())
    }

    #[test]
    fn new_fields_default_for_old_files() {
        for doc in ["{}", r#"{"schema":1,"injectClaudePlugin":true}"#] {
            let s: SpawnSettings = serde_json::from_str(doc).expect("parses");
            assert_eq!(s.env_profiles.len(), 0);
            assert_eq!(s.active_env_profile, None);
        }
    }

    #[test]
    fn a_profile_with_missing_fields_still_parses() {
        let s: SpawnSettings =
            serde_json::from_str(r#"{"envProfiles":[{"name":"a","vars":[{"key":"X"}]}]}"#)
                .expect("parses");
        let var = &s.env_profiles[0].vars[0];
        assert_eq!(
            (var.key.as_str(), var.value.as_str(), var.unset),
            ("X", "", false)
        );
    }

    #[test]
    fn a_profile_round_trips() {
        let mut s = SpawnSettings::default();
        s.env_profiles.push(EnvProfile {
            name: "work".to_owned(),
            vars: vec![
                ProfileVar {
                    key: "A".to_owned(),
                    value: "1".to_owned(),
                    unset: false,
                },
                ProfileVar {
                    key: "HTTP_PROXY".to_owned(),
                    value: String::new(),
                    unset: true,
                },
            ],
        });
        s.active_env_profile = Some("work".to_owned());
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"envProfiles\"") && json.contains("\"activeEnvProfile\""));
        let back: SpawnSettings = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, s);
    }

    #[test]
    fn resolution_order() {
        let mut s = SpawnSettings::default();
        assert!(s.resolve_env_profile(None).is_none());
        s.env_profiles = vec![profile("a", "1"), profile("b", "2"), profile("a", "dup")];
        assert!(s.resolve_env_profile(None).is_none());
        assert!(
            s.resolve_env_profile(Some("a"))
                .is_some_and(|p| p.vars[0].value == "1"),
            "duplicate names: first wins"
        );
        s.active_env_profile = Some("b".to_owned());
        assert_eq!(name_of(s.resolve_env_profile(None)), Some("b"));
        assert_eq!(name_of(s.resolve_env_profile(Some(" a "))), Some("a"));
        assert_eq!(name_of(s.resolve_env_profile(Some("nope"))), Some("b"));
        assert_eq!(name_of(s.resolve_env_profile(Some("A"))), Some("b"));
        s.active_env_profile = Some("gone".to_owned());
        assert!(s.resolve_env_profile(None).is_none());
    }
}
