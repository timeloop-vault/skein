//! Env profiles (#615) in `apply_env`: layering over `extra_env` and the
//! login-shell overlay, unset, per-harness override, reserved keys.

use super::env::{AppliedEnv, apply_env};
use super::probe::ProbeOutcome;
use crate::agent_api::state::HarnessIdentity;
use crate::harness_config::Injection;
use crate::spawn_settings::env_profile::ProfileVar;
use crate::spawn_settings::{EnvProfile, EnvVar, SpawnSettings};
use portable_pty::CommandBuilder;

fn captured(env: Vec<(&str, &str)>) -> ProbeOutcome {
    ProbeOutcome::Captured {
        path: std::env::temp_dir().to_string_lossy().into_owned(),
        env: Some(
            env.into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        ),
        shell: "/bin/zsh".to_owned(),
        elapsed_ms: 12,
    }
}

fn set(key: &str, value: &str) -> ProfileVar {
    ProfileVar {
        key: key.to_owned(),
        value: value.to_owned(),
        unset: false,
    }
}

fn unset(key: &str) -> ProfileVar {
    ProfileVar {
        key: key.to_owned(),
        value: String::new(),
        unset: true,
    }
}

fn profile(name: &str, vars: Vec<ProfileVar>) -> EnvProfile {
    EnvProfile {
        name: name.to_owned(),
        vars,
    }
}

fn settings(profiles: Vec<EnvProfile>, active: Option<&str>) -> SpawnSettings {
    SpawnSettings {
        env_profiles: profiles,
        active_env_profile: active.map(str::to_owned),
        ..SpawnSettings::default()
    }
}

fn run(
    builder: &mut CommandBuilder,
    settings: &SpawnSettings,
    probe: ProbeOutcome,
    over: Option<&str>,
) -> AppliedEnv {
    apply_env(builder, settings, probe, None, &Injection::default(), over)
}

fn get(builder: &CommandBuilder, key: &str) -> Option<String> {
    builder
        .get_env(key)
        .map(|v| v.to_string_lossy().into_owned())
}

#[test]
fn profile_set_overrides_extra_env_and_the_login_overlay() {
    let mut s = settings(
        vec![profile(
            "p",
            vec![set("SKEIN_T_PROF_A", "prof"), set("SKEIN_T_PROF_B", "prof")],
        )],
        Some("p"),
    );
    s.extra_env.push(EnvVar {
        key: "SKEIN_T_PROF_A".to_owned(),
        value: "extra".to_owned(),
    });
    let mut builder = CommandBuilder::new("skein-preview");
    let probe = captured(vec![("SKEIN_T_PROF_B", "rc")]);
    let applied = run(&mut builder, &s, probe, None);
    assert_eq!(get(&builder, "SKEIN_T_PROF_A").as_deref(), Some("prof"));
    assert_eq!(get(&builder, "SKEIN_T_PROF_B").as_deref(), Some("prof"));
    assert!(
        !applied.login_env_keys.iter().any(|k| k == "SKEIN_T_PROF_B"),
        "a profile-owned key is not 'from the login shell'"
    );
}

#[test]
fn unset_removes_inherited_and_login_overlay_vars() {
    let s = settings(
        vec![profile(
            "p",
            vec![unset("SKEIN_T_PROF_INH"), unset("SKEIN_T_PROF_RC")],
        )],
        Some("p"),
    );
    let mut builder = CommandBuilder::new("skein-preview");
    builder.env("SKEIN_T_PROF_INH", "inherited");
    let probe = captured(vec![("SKEIN_T_PROF_RC", "from-rc")]);
    let applied = run(&mut builder, &s, probe, None);
    assert_eq!(get(&builder, "SKEIN_T_PROF_INH"), None);
    assert_eq!(get(&builder, "SKEIN_T_PROF_RC"), None);
    assert_eq!(applied.login_env_keys, Vec::<String>::new());
}

#[test]
fn harness_override_beats_the_active_profile() {
    let s = settings(
        vec![
            profile("a", vec![set("SKEIN_T_PROF_C", "from-a")]),
            profile("b", vec![set("SKEIN_T_PROF_C", "from-b")]),
        ],
        Some("a"),
    );
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = run(&mut builder, &s, captured(vec![]), Some("b"));
    assert_eq!(get(&builder, "SKEIN_T_PROF_C").as_deref(), Some("from-b"));
    assert_eq!(applied.env_profile.as_deref(), Some("b"));
}

#[test]
fn unknown_override_falls_back_to_the_active_profile() {
    let s = settings(
        vec![profile("a", vec![set("SKEIN_T_PROF_D", "from-a")])],
        Some("a"),
    );
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = run(&mut builder, &s, captured(vec![]), Some("missing"));
    assert_eq!(get(&builder, "SKEIN_T_PROF_D").as_deref(), Some("from-a"));
    assert_eq!(applied.env_profile.as_deref(), Some("a"));
}

#[test]
fn no_active_and_no_override_changes_nothing() {
    let s = settings(vec![profile("a", vec![set("SKEIN_T_PROF_E", "x")])], None);
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = run(&mut builder, &s, captured(vec![]), None);
    assert_eq!(get(&builder, "SKEIN_T_PROF_E"), None);
    assert_eq!(applied.env_profile, None);
}

#[test]
fn reserved_keys_are_refused_for_set_and_unset() {
    let s = settings(
        vec![profile(
            "p",
            vec![
                set("PATH", "/evil"),
                unset("PATH"),
                set("SKEIN_ROOM_ID", "other-room"),
                unset("SKEIN_ROOM_ID"),
            ],
        )],
        Some("p"),
    );
    let identity = HarnessIdentity {
        url: "http://127.0.0.1:1".to_owned(),
        token: "t".to_owned(),
        room_id: "room-1".to_owned(),
        harness_id: "h-1".to_owned(),
    };
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = apply_env(
        &mut builder,
        &s,
        captured(vec![]),
        Some(&identity),
        &Injection::default(),
        None,
    );
    assert_eq!(get(&builder, "SKEIN_ROOM_ID").as_deref(), Some("room-1"));
    let path = get(&builder, "PATH").expect("PATH survives");
    assert_ne!(path, "/evil");
    assert_eq!(applied.ignored_env_keys, vec!["PATH", "SKEIN_ROOM_ID"]);
}

#[test]
fn unsetting_no_proxy_still_leaves_loopback() {
    let s = settings(vec![profile("p", vec![unset("NO_PROXY")])], Some("p"));
    let mut builder = CommandBuilder::new("skein-preview");
    builder.env("NO_PROXY", "corp.example");
    let probe = captured(vec![("NO_PROXY", "internal.example")]);
    run(&mut builder, &s, probe, None);
    let value = get(&builder, "NO_PROXY").expect("NO_PROXY is set");
    let entries: Vec<&str> = value.split(',').collect();
    assert!(entries.contains(&"127.0.0.1"), "{value}");
    assert!(
        !entries.iter().any(|e| e.contains("example")),
        "unset dropped the user's entries: {value}"
    );
}

#[test]
fn a_profile_can_unset_a_key_the_global_extra_env_set() {
    let mut s = settings(vec![profile("p", vec![unset("SKEIN_T_PROF_X")])], Some("p"));
    s.extra_env.push(EnvVar {
        key: "SKEIN_T_PROF_X".to_owned(),
        value: "global".to_owned(),
    });
    let mut builder = CommandBuilder::new("skein-preview");
    run(&mut builder, &s, captured(vec![]), None);
    assert_eq!(get(&builder, "SKEIN_T_PROF_X"), None);
}

#[test]
fn applied_key_lists_exclude_refused_keys_and_carry_no_values() {
    let s = settings(
        vec![profile(
            "p",
            vec![
                set("SKEIN_T_PROF_S", "secret-value"),
                unset("SKEIN_T_PROF_U"),
                set("PATH", "/evil"),
                unset("SKEIN_ROOM_ID"),
                set("  ", "blank"),
            ],
        )],
        Some("p"),
    );
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = run(&mut builder, &s, captured(vec![]), None);
    assert_eq!(applied.profile_set_keys, vec!["SKEIN_T_PROF_S"]);
    assert_eq!(applied.profile_unset_keys, vec!["SKEIN_T_PROF_U"]);
    let none = run(
        &mut CommandBuilder::new("skein-preview"),
        &settings(vec![], None),
        captured(vec![]),
        None,
    );
    assert!(none.profile_set_keys.is_empty() && none.profile_unset_keys.is_empty());
}

#[cfg(windows)]
#[test]
fn unset_matches_env_keys_case_insensitively_on_windows() {
    let s = settings(vec![profile("p", vec![unset("http_proxy")])], Some("p"));
    let mut builder = CommandBuilder::new("skein-preview");
    builder.env("HTTP_PROXY", "http://proxy.example");
    let probe = captured(vec![("HTTP_PROXY", "http://rc.example")]);
    run(&mut builder, &s, probe, None);
    assert_eq!(get(&builder, "HTTP_PROXY"), None);
    assert_eq!(get(&builder, "http_proxy"), None);
}

#[test]
fn a_key_set_twice_is_reported_once_with_the_last_value() {
    let s = settings(
        vec![profile(
            "p",
            vec![
                set("SKEIN_T_DUP_A", "first"),
                set("SKEIN_T_DUP_B", "b"),
                set("SKEIN_T_DUP_A", "second"),
            ],
        )],
        Some("p"),
    );
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = run(&mut builder, &s, captured(vec![]), None);
    assert_eq!(
        applied.profile_set_keys,
        vec!["SKEIN_T_DUP_B", "SKEIN_T_DUP_A"]
    );
    assert_eq!(get(&builder, "SKEIN_T_DUP_A").as_deref(), Some("second"));
}

#[test]
fn the_last_of_set_and_unset_decides_which_list_a_key_is_in() {
    let s = settings(
        vec![profile(
            "p",
            vec![
                set("SKEIN_T_DUP_SU", "x"),
                unset("SKEIN_T_DUP_SU"),
                unset("SKEIN_T_DUP_US"),
                set("SKEIN_T_DUP_US", "y"),
            ],
        )],
        Some("p"),
    );
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = run(&mut builder, &s, captured(vec![]), None);
    assert_eq!(applied.profile_set_keys, vec!["SKEIN_T_DUP_US"]);
    assert_eq!(applied.profile_unset_keys, vec!["SKEIN_T_DUP_SU"]);
    assert_eq!(get(&builder, "SKEIN_T_DUP_SU"), None);
    assert_eq!(get(&builder, "SKEIN_T_DUP_US").as_deref(), Some("y"));
}

#[cfg(windows)]
#[test]
fn case_variants_are_one_key_on_windows_and_the_winner_keeps_its_spelling() {
    let s = settings(
        vec![profile(
            "p",
            vec![set("Skein_T_Case", "first"), set("SKEIN_T_CASE", "second")],
        )],
        Some("p"),
    );
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = run(&mut builder, &s, captured(vec![]), None);
    assert_eq!(applied.profile_set_keys, vec!["SKEIN_T_CASE"]);
    assert_eq!(get(&builder, "Skein_T_Case").as_deref(), Some("second"));
}

#[cfg(not(windows))]
#[test]
fn case_variants_stay_distinct_on_unix() {
    let s = settings(
        vec![profile(
            "p",
            vec![set("Skein_T_Case", "first"), set("SKEIN_T_CASE", "second")],
        )],
        Some("p"),
    );
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = run(&mut builder, &s, captured(vec![]), None);
    assert_eq!(
        applied.profile_set_keys,
        vec!["Skein_T_Case", "SKEIN_T_CASE"]
    );
    assert_eq!(get(&builder, "Skein_T_Case").as_deref(), Some("first"));
    assert_eq!(get(&builder, "SKEIN_T_CASE").as_deref(), Some("second"));
}
