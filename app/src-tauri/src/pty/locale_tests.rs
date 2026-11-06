//! The default `LANG` in `apply_env`: set only when the child would
//! otherwise have no locale at all, never over one that is already there.

use super::env::apply_env;
use super::probe::ProbeOutcome;
use crate::harness_config::Injection;
use crate::spawn_settings::env_profile::ProfileVar;
use crate::spawn_settings::{EnvProfile, EnvVar, SpawnSettings};
use portable_pty::CommandBuilder;

const LOCALE_KEYS: &[&str] = &["LANG", "LC_ALL", "LC_CTYPE"];

/// A builder whose inherited env carries exactly `inherited` among the
/// locale keys — the test process's own locale is removed first.
fn builder(inherited: &[(&str, &str)]) -> CommandBuilder {
    let mut b = CommandBuilder::new("x");
    for key in LOCALE_KEYS {
        b.env_remove(key);
    }
    for (k, v) in inherited {
        b.env(k, v);
    }
    b
}

fn probe(login_env: &[(&str, &str)]) -> ProbeOutcome {
    ProbeOutcome::Captured {
        path: std::env::temp_dir().to_string_lossy().into_owned(),
        env: Some(
            login_env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        ),
        shell: "/bin/zsh".to_owned(),
        elapsed_ms: 12,
    }
}

/// The child's locale keys after `apply_env`, in `LOCALE_KEYS` order.
fn locale(
    b: &mut CommandBuilder,
    settings: &SpawnSettings,
    probe: ProbeOutcome,
) -> Vec<Option<String>> {
    apply_env(b, settings, probe, None, &Injection::default(), None);
    LOCALE_KEYS
        .iter()
        .map(|k| b.get_env(k).map(|v| v.to_string_lossy().into_owned()))
        .collect()
}

#[test]
fn no_locale_anywhere_gets_lang_and_nothing_else() {
    let mut b = builder(&[]);
    let got = locale(&mut b, &SpawnSettings::default(), probe(&[]));
    assert_eq!(got, vec![Some("en_US.UTF-8".to_owned()), None, None]);
}

#[test]
fn an_empty_lang_counts_as_unset() {
    let mut b = builder(&[("LANG", "")]);
    let got = locale(&mut b, &SpawnSettings::default(), probe(&[]));
    assert_eq!(got, vec![Some("en_US.UTF-8".to_owned()), None, None]);
}

#[test]
fn any_locale_already_set_is_left_alone() {
    let cases: &[(&str, &str)] = &[
        ("LANG", "sv_SE.UTF-8"),
        ("LC_ALL", "sv_SE.UTF-8"),
        ("LC_CTYPE", "UTF-8"),
    ];
    for &(key, value) in cases {
        let mut b = builder(&[(key, value)]);
        let got = locale(&mut b, &SpawnSettings::default(), probe(&[]));
        let want: Vec<Option<String>> = LOCALE_KEYS
            .iter()
            .map(|k| (*k == key).then(|| value.to_owned()))
            .collect();
        assert_eq!(got, want, "{key} set");
    }
}

#[test]
fn a_locale_from_the_login_shell_or_extra_env_wins() {
    let mut b = builder(&[]);
    let got = locale(
        &mut b,
        &SpawnSettings::default(),
        probe(&[("LC_ALL", "sv_SE.UTF-8")]),
    );
    assert_eq!(got, vec![None, Some("sv_SE.UTF-8".to_owned()), None]);

    let settings = SpawnSettings {
        extra_env: vec![EnvVar {
            key: "LANG".to_owned(),
            value: "sv_SE.UTF-8".to_owned(),
        }],
        ..SpawnSettings::default()
    };
    let mut b = builder(&[]);
    let got = locale(&mut b, &settings, probe(&[]));
    assert_eq!(got, vec![Some("sv_SE.UTF-8".to_owned()), None, None]);
}

#[test]
fn a_profile_that_unsets_lang_keeps_it_unset() {
    let settings = SpawnSettings {
        env_profiles: vec![EnvProfile {
            name: "p".to_owned(),
            vars: vec![ProfileVar {
                key: "LANG".to_owned(),
                value: String::new(),
                unset: true,
            }],
        }],
        active_env_profile: Some("p".to_owned()),
        ..SpawnSettings::default()
    };
    let mut b = builder(&[]);
    let got = locale(&mut b, &settings, probe(&[]));
    assert_eq!(got, vec![None, None, None]);
}
