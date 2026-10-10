//! The login-shell environment overlay in `apply_env`: precedence against
//! the inherited env, the user's extra env and Skein's own variables, plus
//! the `NO_PROXY` loopback guarantee.

use super::env::{AppliedEnv, apply_env};
use super::probe::{ProbeFailure, ProbeOutcome};
use crate::harness_config::Injection;
use crate::spawn_settings::{EnvVar, SpawnSettings};
use portable_pty::CommandBuilder;

fn captured(env: Option<Vec<(&str, &str)>>) -> ProbeOutcome {
    ProbeOutcome::Captured {
        // An existing directory, so `merge_path` keeps it.
        path: std::env::temp_dir().to_string_lossy().into_owned(),
        env: env.map(|v| {
            v.into_iter()
                .map(|(k, val)| (k.to_owned(), val.to_owned()))
                .collect()
        }),
        shell: "/bin/zsh".to_owned(),
        elapsed_ms: 12,
    }
}

fn run(builder: &mut CommandBuilder, settings: &SpawnSettings, probe: ProbeOutcome) -> AppliedEnv {
    apply_env(builder, settings, probe, None, &Injection::default())
}

fn get(builder: &CommandBuilder, key: &str) -> Option<String> {
    builder
        .get_env(key)
        .map(|v| v.to_string_lossy().into_owned())
}

fn extra(key: &str, value: &str) -> EnvVar {
    EnvVar {
        key: key.to_owned(),
        value: value.to_owned(),
    }
}

/// A builder with `NO_PROXY` / `no_proxy` cleared, since it is seeded from
/// the real process environment.
fn clean_proxy_builder() -> CommandBuilder {
    let mut builder = CommandBuilder::new("skein-preview");
    builder.env_remove("NO_PROXY");
    builder.env_remove("no_proxy");
    builder
}

fn entries(value: &str) -> Vec<&str> {
    value.split(',').collect()
}

#[test]
fn a_captured_var_overrides_the_inherited_one() {
    let mut builder = CommandBuilder::new("skein-preview");
    builder.env("SKEIN_TEST_LOGIN_VAR_A", "inherited");
    let probe = captured(Some(vec![("SKEIN_TEST_LOGIN_VAR_A", "from-rc")]));
    run(&mut builder, &SpawnSettings::default(), probe);
    assert_eq!(
        get(&builder, "SKEIN_TEST_LOGIN_VAR_A").as_deref(),
        Some("from-rc")
    );
}

#[test]
fn a_captured_var_nobody_set_reaches_the_child() {
    let mut builder = CommandBuilder::new("skein-preview");
    let probe = captured(Some(vec![("SKEIN_TEST_LOGIN_VAR_B", "api-key")]));
    run(&mut builder, &SpawnSettings::default(), probe);
    assert_eq!(
        get(&builder, "SKEIN_TEST_LOGIN_VAR_B").as_deref(),
        Some("api-key")
    );
}

#[test]
fn user_extra_env_overrides_a_captured_var() {
    let mut settings = SpawnSettings::default();
    settings
        .extra_env
        .push(extra("SKEIN_TEST_LOGIN_VAR_C", "from-settings"));
    let mut builder = CommandBuilder::new("skein-preview");
    let probe = captured(Some(vec![("SKEIN_TEST_LOGIN_VAR_C", "from-rc")]));
    run(&mut builder, &settings, probe);
    assert_eq!(
        get(&builder, "SKEIN_TEST_LOGIN_VAR_C").as_deref(),
        Some("from-settings")
    );
}

#[test]
fn skeins_own_variables_beat_captured_ones() {
    let mut builder = CommandBuilder::new("skein-preview");
    let probe = captured(Some(vec![
        ("TERM", "dumb"),
        ("COLORTERM", "nope"),
        ("SKEIN_VERSION", "0.0.0-rc"),
    ]));
    run(&mut builder, &SpawnSettings::default(), probe);
    assert_eq!(get(&builder, "TERM").as_deref(), Some("xterm-256color"));
    assert_eq!(get(&builder, "COLORTERM").as_deref(), Some("truecolor"));
    assert_eq!(
        get(&builder, "SKEIN_VERSION").as_deref(),
        Some(crate::build_info::VERSION)
    );
}

#[test]
fn probe_session_noise_is_not_applied() {
    let mut builder = CommandBuilder::new("skein-preview");
    builder.env("SHLVL", "inherited-shlvl");
    builder.env("PWD", "/inherited/pwd");
    builder.env("OLDPWD", "/inherited/oldpwd");
    builder.env("_", "inherited-underscore");
    let probe = captured(Some(vec![
        ("SHLVL", "99"),
        ("PWD", "/probe/pwd"),
        ("OLDPWD", "/probe/oldpwd"),
        ("_", "/usr/bin/env"),
        ("PATH", "/captured/overlay/only"),
        ("SKEIN_PROBE", "1"),
    ]));
    let applied = run(&mut builder, &SpawnSettings::default(), probe);

    assert_eq!(get(&builder, "SHLVL").as_deref(), Some("inherited-shlvl"));
    assert_eq!(get(&builder, "PWD").as_deref(), Some("/inherited/pwd"));
    assert_eq!(
        get(&builder, "OLDPWD").as_deref(),
        Some("/inherited/oldpwd")
    );
    assert_eq!(get(&builder, "_").as_deref(), Some("inherited-underscore"));
    assert_ne!(get(&builder, "SKEIN_PROBE").as_deref(), Some("1"));

    // PATH comes from the probe's `path` field, never from the overlay.
    let path = get(&builder, "PATH").expect("PATH is set");
    let dirs: Vec<_> = std::env::split_paths(&path).collect();
    assert!(
        !dirs
            .iter()
            .any(|d| d.to_string_lossy() == "/captured/overlay/only"),
        "overlay PATH leaked: {path}"
    );
    assert!(
        dirs.contains(&std::env::temp_dir()),
        "probe path missing: {path}"
    );
    assert_eq!(applied.path.to_string_lossy(), path);
}

#[test]
fn a_captured_host_terminal_var_is_stripped_when_the_setting_is_on() {
    let mut builder = CommandBuilder::new("skein-preview");
    let probe = captured(Some(vec![("TERM_PROGRAM", "iTerm.app")]));
    let applied = run(&mut builder, &SpawnSettings::default(), probe);
    assert_eq!(get(&builder, "TERM_PROGRAM"), None);
    assert!(applied.stripped.iter().any(|k| k == "TERM_PROGRAM"));
}

#[test]
fn a_captured_host_terminal_var_is_kept_when_the_setting_is_off() {
    let settings = SpawnSettings {
        strip_host_env: false,
        ..SpawnSettings::default()
    };
    let mut builder = CommandBuilder::new("skein-preview");
    let probe = captured(Some(vec![("TERM_PROGRAM", "iTerm.app")]));
    let applied = run(&mut builder, &settings, probe);
    assert_eq!(get(&builder, "TERM_PROGRAM").as_deref(), Some("iTerm.app"));
    assert_eq!(applied.stripped, Vec::<String>::new());
    assert!(applied.login_env_keys.iter().any(|k| k == "TERM_PROGRAM"));
}

#[test]
fn login_env_keys_are_sorted_names_without_noise_stripped_or_reserved() {
    let mut builder = CommandBuilder::new("skein-preview");
    let probe = captured(Some(vec![
        ("SKEIN_TEST_LOGIN_VAR_Z", "secret-z"),
        ("SKEIN_TEST_LOGIN_VAR_M", "secret-m"),
        ("SKEIN_TEST_LOGIN_VAR_A", "secret-a"),
        // noise
        ("SHLVL", "2"),
        ("PWD", "/x"),
        ("OLDPWD", "/y"),
        ("_", "/usr/bin/env"),
        ("PATH", "/p"),
        ("SKEIN_PROBE", "1"),
        ("DISABLE_AUTO_UPDATE", "true"),
        // stripped as host-terminal identity
        ("TERM_PROGRAM", "iTerm.app"),
        // reserved: Skein overwrites them
        ("TERM", "dumb"),
        ("COLORTERM", "x"),
        ("SHELL", "/bin/fish"),
        ("SKEIN_VERSION", "0"),
        ("SKEIN_ROOM_ID", "r"),
    ]));
    let applied = run(&mut builder, &SpawnSettings::default(), probe);
    assert_eq!(
        applied.login_env_keys,
        [
            "SKEIN_TEST_LOGIN_VAR_A",
            "SKEIN_TEST_LOGIN_VAR_M",
            "SKEIN_TEST_LOGIN_VAR_Z",
        ]
    );
    let joined = applied.login_env_keys.join(",");
    assert!(!joined.contains("secret"), "values leaked: {joined}");
}

#[test]
fn login_env_keys_exclude_names_the_user_overrides() {
    let mut builder = CommandBuilder::new("skein-preview");
    let probe = captured(Some(vec![
        ("SKEIN_TEST_LOGIN_VAR_X", "from-shell"),
        ("SKEIN_TEST_LOGIN_VAR_Y", "from-shell"),
    ]));
    let settings = SpawnSettings {
        extra_env: vec![extra("  skein_test_login_var_x ", "mine")],
        ..SpawnSettings::default()
    };
    let applied = run(&mut builder, &settings, probe);
    assert_eq!(applied.login_env_keys, ["SKEIN_TEST_LOGIN_VAR_Y"]);
}

#[test]
fn probe_outcome_debug_never_shows_env_values() {
    let probe = captured(Some(vec![("API_KEY", "hunter2-secret")]));
    let shown = format!("{probe:?}");
    assert!(!shown.contains("hunter2"), "value leaked: {shown}");
    assert!(shown.contains("<1 vars>"), "{shown}");
}

#[test]
fn a_path_only_capture_overlays_nothing() {
    let mut builder = CommandBuilder::new("skein-preview");
    builder.env("SKEIN_TEST_LOGIN_VAR_D", "inherited");
    let applied = run(&mut builder, &SpawnSettings::default(), captured(None));
    assert_eq!(applied.login_env_keys, Vec::<String>::new());
    assert_eq!(
        get(&builder, "SKEIN_TEST_LOGIN_VAR_D").as_deref(),
        Some("inherited")
    );
    // The probe's PATH still applies.
    let path = get(&builder, "PATH").expect("PATH is set");
    assert!(std::env::split_paths(&path).any(|d| d == std::env::temp_dir()));
}

#[test]
fn a_failed_probe_overlays_nothing() {
    let mut builder = CommandBuilder::new("skein-preview");
    builder.env("SKEIN_TEST_LOGIN_VAR_E", "inherited");
    let probe = ProbeOutcome::Failed {
        reason: ProbeFailure::Timeout,
        shell: "/bin/zsh".to_owned(),
        elapsed_ms: 5000,
    };
    let applied = run(&mut builder, &SpawnSettings::default(), probe);
    assert_eq!(applied.login_env_keys, Vec::<String>::new());
    assert_eq!(
        get(&builder, "SKEIN_TEST_LOGIN_VAR_E").as_deref(),
        Some("inherited")
    );
}

#[test]
fn no_proxy_unset_gets_the_loopback_hosts() {
    let mut builder = clean_proxy_builder();
    run(&mut builder, &SpawnSettings::default(), captured(None));
    let value = get(&builder, "NO_PROXY").expect("NO_PROXY is set");
    assert_eq!(entries(&value), ["127.0.0.1", "localhost", "::1"]);
}

#[test]
fn an_existing_no_proxy_is_kept_first_and_loopback_appended() {
    let mut builder = clean_proxy_builder();
    builder.env("NO_PROXY", "corp.example");
    run(&mut builder, &SpawnSettings::default(), captured(None));
    let value = get(&builder, "NO_PROXY").expect("NO_PROXY is set");
    assert_eq!(
        entries(&value),
        ["corp.example", "127.0.0.1", "localhost", "::1"]
    );
}

#[test]
fn a_captured_no_proxy_is_merged_with_loopback() {
    let mut builder = clean_proxy_builder();
    let probe = captured(Some(vec![("NO_PROXY", "rc.example,localhost")]));
    run(&mut builder, &SpawnSettings::default(), probe);
    let value = get(&builder, "NO_PROXY").expect("NO_PROXY is set");
    assert_eq!(
        entries(&value),
        ["rc.example", "localhost", "127.0.0.1", "::1"]
    );
}

#[test]
fn a_user_no_proxy_entry_is_merged_not_replaced() {
    let mut settings = SpawnSettings::default();
    settings
        .extra_env
        .push(extra("no_proxy", "internal.example, 10.0.0.0/8"));
    let mut builder = clean_proxy_builder();
    let probe = captured(Some(vec![("NO_PROXY", "rc.example")]));
    let applied = run(&mut builder, &settings, probe);
    assert_eq!(applied.ignored_env_keys, Vec::<String>::new());
    let value = get(&builder, "NO_PROXY").expect("NO_PROXY is set");
    let list = entries(&value);
    for want in [
        "internal.example",
        "10.0.0.0/8",
        "127.0.0.1",
        "localhost",
        "::1",
    ] {
        assert!(list.contains(&want), "{want} missing from {value}");
    }
    // On Unix the captured uppercase list survives beside the user's
    // lowercase one; on Windows both names are one entry, so the user's
    // value (written later) is the one that replaces it before merging.
    #[cfg(not(windows))]
    assert!(list.contains(&"rc.example"), "{value}");
}

#[test]
fn no_proxy_has_no_duplicate_entries() {
    let mut builder = clean_proxy_builder();
    builder.env(
        "NO_PROXY",
        "localhost,LOCALHOST,127.0.0.1, corp.example,corp.example",
    );
    run(&mut builder, &SpawnSettings::default(), captured(None));
    let value = get(&builder, "NO_PROXY").expect("NO_PROXY is set");
    let mut seen: Vec<String> = Vec::new();
    for entry in entries(&value) {
        let lower = entry.to_ascii_lowercase();
        assert!(!seen.contains(&lower), "duplicate {entry} in {value}");
        seen.push(lower);
    }
    assert_eq!(
        entries(&value),
        ["localhost", "127.0.0.1", "corp.example", "::1"]
    );
}

#[cfg(not(windows))]
#[test]
fn on_unix_no_proxy_and_its_lowercase_twin_are_equal() {
    let mut builder = clean_proxy_builder();
    builder.env("no_proxy", "lower.example");
    builder.env("NO_PROXY", "upper.example");
    run(&mut builder, &SpawnSettings::default(), captured(None));
    let upper = get(&builder, "NO_PROXY").expect("NO_PROXY is set");
    let lower = get(&builder, "no_proxy").expect("no_proxy is set");
    assert_eq!(upper, lower);
    assert!(upper.contains("upper.example") && upper.contains("lower.example"));
}

#[cfg(windows)]
#[test]
fn on_windows_no_proxy_is_a_single_case_insensitive_entry() {
    let mut builder = clean_proxy_builder();
    run(&mut builder, &SpawnSettings::default(), captured(None));
    assert_eq!(get(&builder, "NO_PROXY"), get(&builder, "no_proxy"));
    let count = builder
        .iter_full_env_as_str()
        .filter(|(k, _)| k.eq_ignore_ascii_case("no_proxy"))
        .count();
    assert_eq!(count, 1);
}

#[test]
fn describe_counts_the_captured_vars() {
    let text = captured(Some(vec![("A", "1"), ("B", "2"), ("C", "3")])).describe();
    assert!(text.contains("PATH + 3 vars"), "{text}");
    assert!(text.contains("/bin/zsh"), "{text}");
}

#[test]
fn describe_says_path_only_when_the_env_capture_failed() {
    let text = captured(None).describe();
    assert!(text.contains("PATH only"), "{text}");
    assert!(text.contains("env capture failed"), "{text}");
}
