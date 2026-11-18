use super::*;

/// Every test here predates the v2 file and means the v1 shape.
fn injection_for(
    kind: HarnessKind,
    program: &str,
    config: Option<&HarnessConfig>,
    settings: &SpawnSettings,
    agent: Option<&HarnessIdentity>,
) -> Injection {
    super::injection_for(
        kind,
        program,
        config,
        settings,
        agent,
        OpencodeConfigShape::V1,
    )
}

pub(super) fn identity() -> HarnessIdentity {
    HarnessIdentity {
        url: "http://127.0.0.1:4242/mcp".to_owned(),
        token: "tok".to_owned(),
        room_id: "s_1".to_owned(),
        harness_id: "h_1".to_owned(),
    }
}

pub(super) fn bundle() -> (tempfile::TempDir, HarnessConfig) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join(BUNDLE_DIR);
    let manifest = root.join(CLAUDE_PLUGIN_SUBDIR).join(CLAUDE_PLUGIN_MANIFEST);
    std::fs::create_dir_all(manifest.parent().expect("manifest parent")).expect("mkdir");
    std::fs::write(&manifest, "{}").expect("write manifest");
    let oc = root.join(OPENCODE_CONFIG_FILE);
    std::fs::create_dir_all(oc.parent().expect("oc parent")).expect("mkdir");
    std::fs::write(&oc, "{}").expect("write opencode config");
    std::fs::write(root.join(OPENCODE_CONFIG_V2_FILE), "{}").expect("write opencode v2 config");
    for rel in [HERMES_HOOK_PY_FILE, HERMES_HOOK_SH_FILE] {
        let hook = root.join(rel);
        std::fs::create_dir_all(hook.parent().expect("hook parent")).expect("mkdir");
        std::fs::write(&hook, "").expect("write hermes hook");
    }
    let config = HarnessConfig::resolve(dir.path());
    (dir, config)
}

#[test]
fn a_complete_bundle_resolves_both_halves() {
    let (_tmp, config) = bundle();
    assert!(config.error.is_none());
    assert!(config.claude_plugin.is_some());
    assert!(config.opencode_config.is_some());
    assert!(config.opencode_config_v2.is_some());
    assert!(config.hermes_hook_py.is_some());
    assert!(config.hermes_hook_sh.is_some());
}

#[test]
fn missing_hermes_hooks_resolve_to_none_and_are_reported() {
    let (tmp, _) = bundle();
    std::fs::remove_file(tmp.path().join(BUNDLE_DIR).join(HERMES_HOOK_SH_FILE))
        .expect("remove hook");
    let config = HarnessConfig::resolve(tmp.path());
    assert!(config.hermes_hook_py.is_some());
    assert!(config.hermes_hook_sh.is_none());
    assert!(config.claude_plugin.is_some());
    let error = config.error.expect("missing hook is reported");
    assert!(error.contains("skein-hook.sh"), "{error}");
}

#[test]
fn a_missing_bundle_reports_which_paths_it_looked_for() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = HarnessConfig::resolve(dir.path());
    let error = config
        .error
        .expect("an empty resource dir must report an error");
    assert!(error.contains(CLAUDE_PLUGIN_MANIFEST), "{error}");
    assert!(error.contains("opencode.json"), "{error}");
}

#[test]
fn claude_gets_the_plugin_dir_and_nothing_in_the_environment() {
    let (_tmp, config) = bundle();
    let injection = injection_for(
        HarnessKind::Claude,
        "claude",
        Some(&config),
        &SpawnSettings::default(),
        Some(&identity()),
    );
    assert_eq!(
        injection.args.first().map(String::as_str),
        Some("--plugin-dir")
    );
    assert!(
        injection
            .args
            .get(1)
            .is_some_and(|p| p.ends_with(CLAUDE_PLUGIN_SUBDIR)),
        "{:?}",
        injection.args
    );
    assert_eq!(injection.env, Vec::new());
}

#[test]
fn opencode_gets_the_config_variable_and_nothing_in_the_argv() {
    let (_tmp, config) = bundle();
    let injection = injection_for(
        HarnessKind::Opencode,
        "opencode",
        Some(&config),
        &SpawnSettings::default(),
        Some(&identity()),
    );
    assert_eq!(injection.args, Vec::<String>::new());
    assert_eq!(
        injection.env.first().map(|(k, _)| k.as_str()),
        Some(OPENCODE_CONFIG_VAR)
    );
}

#[test]
fn kinds_with_no_review_path_get_nothing() {
    // An unknown kind string is unrepresentable now — it is
    // rejected at the Tauri boundary (#116) before it ever reaches
    // this function.
    let (_tmp, config) = bundle();
    for kind in [
        HarnessKind::Copilot,
        HarnessKind::Byoh,
        HarnessKind::Files,
        HarnessKind::Remote,
        HarnessKind::Hermes,
    ] {
        // Real program where the kind has one (`gh` for copilot),
        // so this exercises the `match kind` fallthrough and not
        // just the program-mismatch gate above it.
        let program = kind.program().unwrap_or(kind.as_str());
        let injection = injection_for(
            kind,
            program,
            Some(&config),
            &SpawnSettings::default(),
            Some(&identity()),
        );
        assert!(injection.is_empty(), "{kind} should get no injection");
    }
}

#[test]
fn copilot_reaches_the_match_but_still_gets_nothing() {
    // Copilot's own program is `gh`, so with both injections on and
    // a real program string the gate above the `match kind` opens —
    // this is what proves the `_ => Injection::default()` arm, not
    // just the program-mismatch gate, is what refuses it.
    let (_tmp, config) = bundle();
    let settings = SpawnSettings {
        inject_claude_plugin: true,
        inject_opencode_config: true,
        ..SpawnSettings::default()
    };
    let injection = injection_for(
        HarnessKind::Copilot,
        "gh",
        Some(&config),
        &settings,
        Some(&identity()),
    );
    assert!(injection.is_empty());
}

#[test]
fn each_toggle_turns_off_only_its_own_harness() {
    let (_tmp, config) = bundle();
    let settings = SpawnSettings {
        inject_claude_plugin: false,
        ..SpawnSettings::default()
    };
    assert!(
        injection_for(
            HarnessKind::Claude,
            "claude",
            Some(&config),
            &settings,
            Some(&identity())
        )
        .is_empty()
    );
    assert!(
        !injection_for(
            HarnessKind::Opencode,
            "opencode",
            Some(&config),
            &settings,
            Some(&identity())
        )
        .is_empty(),
        "turning off the Claude plugin must not disable opencode's config"
    );
}

#[test]
fn a_kind_whose_command_was_swapped_gets_no_args() {
    // "Press Enter for shell" spawns the shell argv (launch mode,
    // #752) but not a new kind, so the kind still says `claude`. Appending --plugin-dir to bash
    // would break the spawn outright.
    let (_tmp, config) = bundle();
    let injection = injection_for(
        HarnessKind::Claude,
        "/bin/bash",
        Some(&config),
        &SpawnSettings::default(),
        Some(&identity()),
    );
    assert_eq!(injection.args, Vec::<String>::new());
}

#[test]
fn claude_post_exit_shell_gets_the_env_vars() {
    let (_tmp, config) = bundle();
    let injection = injection_for(
        HarnessKind::Claude,
        "pwsh",
        Some(&config),
        &SpawnSettings::default(),
        Some(&identity()),
    );
    assert_eq!(injection.args, Vec::<String>::new());
    assert_eq!(injection.env.len(), 2);
    let dirs = injection
        .env
        .iter()
        .find(|(k, _)| k == CLAUDE_PLUGIN_DIRS_VAR)
        .expect("plugin dirs var");
    assert!(dirs.1.ends_with(CLAUDE_PLUGIN_SUBDIR), "{dirs:?}");
    assert!(
        injection
            .env
            .iter()
            .any(|(k, v)| k == CLAUDE_PLUGIN_VIA_ENV_VAR && v == "1")
    );
}

#[test]
fn the_real_claude_spawn_gets_no_env_var() {
    let (_tmp, config) = bundle();
    let injection = injection_for(
        HarnessKind::Claude,
        "claude",
        Some(&config),
        &SpawnSettings::default(),
        Some(&identity()),
    );
    assert_eq!(
        injection.args.first().map(String::as_str),
        Some("--plugin-dir")
    );
    assert_eq!(injection.env, Vec::new());
}

#[test]
fn the_post_exit_shell_env_respects_the_toggle_and_the_endpoint() {
    let (_tmp, config) = bundle();
    let off = SpawnSettings {
        inject_claude_plugin: false,
        ..SpawnSettings::default()
    };
    assert!(
        injection_for(
            HarnessKind::Claude,
            "zsh",
            Some(&config),
            &off,
            Some(&identity())
        )
        .is_empty()
    );
    assert!(
        injection_for(
            HarnessKind::Claude,
            "zsh",
            Some(&config),
            &SpawnSettings::default(),
            None
        )
        .is_empty()
    );
}

#[test]
fn an_opencode_post_exit_shell_gets_opencode_config() {
    let (_tmp, config) = bundle();
    let injection = injection_for(
        HarnessKind::Opencode,
        "zsh",
        Some(&config),
        &SpawnSettings::default(),
        Some(&identity()),
    );
    assert_eq!(injection.args, Vec::<String>::new());
    assert_eq!(injection.env.len(), 1);
    assert_eq!(injection.env[0].0, OPENCODE_CONFIG_VAR);
    let off = SpawnSettings {
        inject_opencode_config: false,
        ..SpawnSettings::default()
    };
    assert!(
        injection_for(
            HarnessKind::Opencode,
            "zsh",
            Some(&config),
            &off,
            Some(&identity())
        )
        .is_empty()
    );
    assert!(
        injection_for(
            HarnessKind::Opencode,
            "zsh",
            Some(&config),
            &SpawnSettings::default(),
            None
        )
        .is_empty()
    );
}

#[test]
fn a_non_opencode_shell_gets_no_opencode_config() {
    let (_tmp, config) = bundle();
    let injection = injection_for(
        HarnessKind::Byoh,
        "zsh",
        Some(&config),
        &SpawnSettings::default(),
        Some(&identity()),
    );
    assert!(injection.is_empty());
}

#[test]
fn plugin_dirs_merge_additively_without_duplicating() {
    let sep = if cfg!(windows) { ";" } else { ":" };
    assert_eq!(merge_plugin_dirs(None, "/ours"), "/ours");
    assert_eq!(merge_plugin_dirs(Some("  "), "/ours"), "/ours");
    assert_eq!(
        merge_plugin_dirs(Some("/theirs"), "/ours"),
        format!("/theirs{sep}/ours")
    );
    let already = format!("/theirs{sep}/ours");
    assert_eq!(merge_plugin_dirs(Some(&already), "/ours"), already);
}

/// An absolute path is the normal case once #207's Windows
/// re-resolution has run, and `.cmd` is what an npm-installed
/// harness resolves to there. Backslash separators only parse as
/// separators on Windows, so that case is gated rather than
/// asserted everywhere.
#[test]
fn a_resolved_path_still_counts_as_the_program() {
    let (_tmp, config) = bundle();
    let mut programs = vec!["/opt/homebrew/bin/claude", "claude"];
    if cfg!(windows) {
        programs.push(r"C:\Users\x\.local\bin\claude.cmd");
        programs.push("CLAUDE.EXE");
    }
    for program in programs {
        let injection = injection_for(
            HarnessKind::Claude,
            program,
            Some(&config),
            &SpawnSettings::default(),
            Some(&identity()),
        );
        assert!(!injection.is_empty(), "{program} should be recognised");
    }
}

/// The hook bundle actually shipped, not a copy of it — a
/// hand-transcribed fixture would happily drift from what
/// `tauri.conf.json` bundles while this test kept passing (#86).
#[test]
fn the_shipped_permission_hook_posts_to_the_right_route_and_never_fails_the_turn() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../app/src-tauri")
        .join(BUNDLE_DIR)
        .join(CLAUDE_PLUGIN_SUBDIR)
        .join("hooks/hooks.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");

    let requests = parsed["hooks"]["PermissionRequest"]
        .as_array()
        .expect("a PermissionRequest matcher group");
    let hook = requests
        .iter()
        .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
        .find(|hook| hook["type"] == "command")
        .expect("a command hook under PermissionRequest");

    assert_eq!(
        hook["async"].as_bool(),
        Some(true),
        "a synchronous hook would stall the turn on Skein's own response"
    );
    let command = hook["command"].as_str().expect("command is a string");
    assert!(
        command.contains("${SKEIN_REVIEW_URL%/mcp}/api/harness/permission"),
        "{command}"
    );
    assert!(
        command.trim_end().ends_with("exit 0"),
        "a failing curl must not fail the tool call: {command}"
    );
}

#[test]
fn without_a_bound_endpoint_nothing_is_injected() {
    // The configs interpolate SKEIN_REVIEW_URL, which is only set
    // when the agent API bound. Injecting anyway would register an
    // MCP server pointing at the literal variable name.
    let (_tmp, config) = bundle();
    for kind in [HarnessKind::Claude, HarnessKind::Opencode] {
        let injection = injection_for(
            kind,
            kind.as_str(),
            Some(&config),
            &SpawnSettings::default(),
            None,
        );
        assert!(injection.is_empty(), "{kind} without an endpoint");
    }
}

#[test]
fn claude_wheel_acceleration_is_left_alone_by_default() {
    assert_eq!(
        claude_settings_args(HarnessKind::Claude, "claude", &SpawnSettings::default()),
        Vec::<String>::new()
    );
}

#[test]
fn turning_claude_wheel_acceleration_off_passes_its_setting() {
    let settings = SpawnSettings {
        disable_claude_wheel_acceleration: true,
        ..SpawnSettings::default()
    };
    assert_eq!(
        claude_settings_args(HarnessKind::Claude, "/opt/homebrew/bin/claude", &settings),
        vec![
            "--settings".to_owned(),
            r#"{"wheelScrollAccelerationEnabled":false}"#.to_owned()
        ]
    );
}

#[test]
fn the_wheel_setting_reaches_only_a_real_claude() {
    let settings = SpawnSettings {
        disable_claude_wheel_acceleration: true,
        ..SpawnSettings::default()
    };
    // The post-exit shell keeps the `claude` kind but runs bash, which
    // would choke on `--settings`.
    assert_eq!(
        claude_settings_args(HarnessKind::Claude, "/bin/bash", &settings),
        Vec::<String>::new()
    );
    assert_eq!(
        claude_settings_args(HarnessKind::Opencode, "opencode", &settings),
        Vec::<String>::new()
    );
}
