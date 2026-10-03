use super::HarnessIdentity;
use super::env::{AGENT_IDENTITY_ENV_KEYS, apply_env};
use super::probe::probe_snapshot;
use crate::harness_config::Injection;
use crate::spawn_settings::SpawnSettings;
use portable_pty::CommandBuilder;
use std::collections::HashMap;

#[test]
fn a_harness_is_told_where_its_rooms_review_lives() {
    // #213's whole delivery mechanism. Both harnesses expand
    // ${VAR} in their MCP config, so if these four are missing the
    // agent has no review tools at all — and nothing else in the
    // spawn path would notice.
    let settings = SpawnSettings::default();
    let identity = HarnessIdentity {
        url: "http://127.0.0.1:51234/mcp".to_owned(),
        token: "deadbeef".to_owned(),
        room_id: "s_7f2".to_owned(),
        harness_id: "h_a91".to_owned(),
    };
    let mut builder = CommandBuilder::new("skein-preview");
    apply_env(
        &mut builder,
        &settings,
        probe_snapshot(),
        Some(&identity),
        &Injection::default(),
    );
    let env: HashMap<String, String> = builder
        .iter_full_env_as_str()
        .map(|(k, v)| (k.to_uppercase(), v.to_owned()))
        .collect();
    assert_eq!(
        env.get("SKEIN_REVIEW_URL").map(String::as_str),
        Some("http://127.0.0.1:51234/mcp")
    );
    assert_eq!(
        env.get("SKEIN_REVIEW_TOKEN").map(String::as_str),
        Some("deadbeef")
    );
    assert_eq!(env.get("SKEIN_ROOM_ID").map(String::as_str), Some("s_7f2"));
    assert_eq!(
        env.get("SKEIN_HARNESS_ID").map(String::as_str),
        Some("h_a91")
    );

    // And without an identity — the settings preview, or a boot
    // where the server never bound — nothing is set at all, rather
    // than four variables pointing at a dead port. Seed the base
    // env with a stale set first (#243): that is what a Skein
    // launched from inside another Skein's harness inherits, and
    // it must be removed, not merely left unset.
    let mut bare = CommandBuilder::new("skein-preview");
    bare.env("SKEIN_REVIEW_URL", "http://127.0.0.1:1/mcp");
    bare.env("SKEIN_REVIEW_TOKEN", "outer-room-token");
    bare.env("SKEIN_ROOM_ID", "outer");
    bare.env("SKEIN_HARNESS_ID", "outer-h");
    apply_env(
        &mut bare,
        &settings,
        probe_snapshot(),
        None,
        &Injection::default(),
    );
    let leaked: Vec<String> = bare
        .iter_full_env_as_str()
        .map(|(k, _)| k.to_uppercase())
        .filter(|k| AGENT_IDENTITY_ENV_KEYS.contains(&k.as_str()))
        .collect();
    assert!(leaked.is_empty(), "identity vars survived: {leaked:?}");
}

#[test]
fn the_review_variables_cannot_be_pinned_by_hand() {
    // Setting SKEIN_REVIEW_TOKEN in the user's extra env would point
    // one harness at another room's review. It is refused out loud
    // rather than silently overwritten below.
    let settings = SpawnSettings {
        extra_env: vec![crate::spawn_settings::EnvVar {
            key: "SKEIN_REVIEW_TOKEN".to_owned(),
            value: "somebody-elses".to_owned(),
        }],
        ..SpawnSettings::default()
    };
    let mut builder = CommandBuilder::new("skein-preview");
    let applied = apply_env(
        &mut builder,
        &settings,
        probe_snapshot(),
        None,
        &Injection::default(),
    );
    assert_eq!(applied.ignored_env_keys, vec!["SKEIN_REVIEW_TOKEN"]);
}

#[test]
fn an_injected_variable_is_reserved_only_while_it_is_being_injected() {
    // #215's escape hatch is the whole point of this asymmetry.
    // OPENCODE_CONFIG is ours *when we are writing it*, so a user
    // who pins it by hand is told it was ignored rather than
    // silently losing it. But turning the injection off in Settings
    // is the documented way to reclaim the variable for your own
    // config file — so with nothing injected, the user's value has
    // to reach the child.
    let settings = SpawnSettings {
        extra_env: vec![crate::spawn_settings::EnvVar {
            key: "OPENCODE_CONFIG".to_owned(),
            value: "/home/me/mine.json".to_owned(),
        }],
        ..SpawnSettings::default()
    };
    let injecting = Injection {
        args: Vec::new(),
        env: vec![(
            "OPENCODE_CONFIG".to_owned(),
            "/opt/skein/opencode.json".to_owned(),
        )],
    };

    let mut builder = CommandBuilder::new("skein-preview");
    let applied = apply_env(&mut builder, &settings, probe_snapshot(), None, &injecting);
    assert_eq!(applied.ignored_env_keys, vec!["OPENCODE_CONFIG"]);
    let value = builder
        .iter_full_env_as_str()
        .find(|(k, _)| k.eq_ignore_ascii_case("OPENCODE_CONFIG"))
        .map(|(_, v)| v.to_owned());
    assert_eq!(value.as_deref(), Some("/opt/skein/opencode.json"));

    let mut off = CommandBuilder::new("skein-preview");
    let applied = apply_env(
        &mut off,
        &settings,
        probe_snapshot(),
        None,
        &Injection::default(),
    );
    assert!(
        applied.ignored_env_keys.is_empty(),
        "with the injection off the key is the user's again"
    );
    let value = off
        .iter_full_env_as_str()
        .find(|(k, _)| k.eq_ignore_ascii_case("OPENCODE_CONFIG"))
        .map(|(_, v)| v.to_owned());
    assert_eq!(value.as_deref(), Some("/home/me/mine.json"));
}

/// The final `CLAUDE_CODE_PLUGIN_DIRS` after `apply_env` (#318), with
/// the user's extra env and/or an inherited value seeded first.
fn plugin_dirs_after(inherited: Option<&str>, user: Option<&str>, ours: &str) -> Option<String> {
    use crate::harness_config::CLAUDE_PLUGIN_DIRS_VAR as VAR;
    let mut settings = SpawnSettings::default();
    if let Some(v) = user {
        settings.extra_env.push(crate::spawn_settings::EnvVar {
            key: VAR.to_owned(),
            value: v.to_owned(),
        });
    }
    let injection = Injection {
        env: vec![(VAR.to_owned(), ours.to_owned())],
        ..Injection::default()
    };
    let mut builder = CommandBuilder::new("skein-preview");
    if let Some(v) = inherited {
        builder.env(VAR, v);
    }
    apply_env(&mut builder, &settings, probe_snapshot(), None, &injection);
    builder
        .iter_full_env_as_str()
        .find(|(k, _)| k.eq_ignore_ascii_case(VAR))
        .map(|(_, v)| v.to_owned())
}

const SEP: &str = if cfg!(windows) { ";" } else { ":" };

#[test]
fn plugin_dirs_is_exactly_ours_with_no_prior_value() {
    assert_eq!(
        plugin_dirs_after(None, None, "/skein/plugin").as_deref(),
        Some("/skein/plugin")
    );
}

#[test]
fn plugin_dirs_keeps_the_users_extra_env_value() {
    let want = format!("/mine{SEP}/skein/plugin");
    assert_eq!(
        plugin_dirs_after(None, Some("/mine"), "/skein/plugin").as_deref(),
        Some(want.as_str())
    );
}

#[test]
fn plugin_dirs_keeps_an_inherited_value() {
    let want = format!("/inherited{SEP}/skein/plugin");
    assert_eq!(
        plugin_dirs_after(Some("/inherited"), None, "/skein/plugin").as_deref(),
        Some(want.as_str())
    );
}

#[test]
fn plugin_dirs_does_not_duplicate_our_dir() {
    let already = format!("/mine{SEP}/skein/plugin");
    assert_eq!(
        plugin_dirs_after(None, Some(&already), "/skein/plugin").as_deref(),
        Some(already.as_str())
    );
}
