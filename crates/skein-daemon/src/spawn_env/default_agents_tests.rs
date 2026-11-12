use std::collections::BTreeMap;

use crate::harness_kind::HarnessKind;

use super::*;
use crate::spawn_env::lookups::spawn_settings_store;
use crate::spawn_settings::{self, SpawnSettings};

fn state(dir: &tempfile::TempDir) -> SpawnEnvState {
    SpawnEnvState {
        data_dir: dir.path().to_path_buf(),
        settings: parking_lot::RwLock::new(SpawnSettings::default()),
        degraded: parking_lot::RwLock::new(None),
    }
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn on_disk(dir: &tempfile::TempDir) -> Option<BTreeMap<String, String>> {
    spawn_settings::load(dir.path()).0.default_agents
}

#[test]
fn old_file_without_the_key_loads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(spawn_settings::file_path(dir.path()), r#"{"schema":1}"#).unwrap();
    assert_eq!(on_disk(&dir), None);
}

#[test]
fn some_map_and_some_empty_round_trip() {
    for value in [map(&[("claude", "reviewer")]), BTreeMap::new()] {
        let dir = tempfile::tempdir().unwrap();
        let s = SpawnSettings {
            default_agents: Some(value.clone()),
            ..SpawnSettings::default()
        };
        spawn_settings::save(dir.path(), &s).unwrap();
        assert_eq!(on_disk(&dir), Some(value));
    }
}

#[test]
fn none_is_not_serialised() {
    let json = serde_json::to_string(&SpawnSettings::default()).unwrap();
    assert!(!json.contains("defaultAgents"));
}

#[test]
fn import_runs_once_and_does_not_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(&dir);
    let first = default_agents_import_impl(&st, map(&[("claude", "a")])).unwrap();
    assert!(first.imported);
    assert_eq!(first.default_agents, map(&[("claude", "a")]));
    let second = default_agents_import_impl(&st, map(&[("opencode", "b")])).unwrap();
    assert!(!second.imported);
    assert_eq!(second.default_agents, map(&[("claude", "a")]));
    assert_eq!(on_disk(&dir), Some(map(&[("claude", "a")])));
}

#[test]
fn import_of_nothing_still_claims_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(&dir);
    assert!(
        default_agents_import_impl(&st, BTreeMap::new())
            .unwrap()
            .imported
    );
    assert!(
        !default_agents_import_impl(&st, map(&[("claude", "a")]))
            .unwrap()
            .imported
    );
    assert_eq!(on_disk(&dir), Some(BTreeMap::new()));
}

#[test]
fn import_drops_blank_names_and_unknown_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(&dir);
    let r = default_agents_import_impl(
        &st,
        map(&[
            ("claude", " plan "),
            ("opencode", "  "),
            ("nope", "x"),
            ("copilot", ""),
        ]),
    )
    .unwrap();
    assert_eq!(r.default_agents, map(&[("claude", "plan")]));
}

#[test]
fn set_adds_replaces_and_removes_even_before_import() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(&dir);
    let m = default_agent_set_impl(&st, HarnessKind::Claude, Some("a")).unwrap();
    assert_eq!(m, map(&[("claude", "a")]));
    let m = default_agent_set_impl(&st, HarnessKind::Opencode, Some("b")).unwrap();
    assert_eq!(m, map(&[("claude", "a"), ("opencode", "b")]));
    let m = default_agent_set_impl(&st, HarnessKind::Claude, Some("c")).unwrap();
    assert_eq!(m, map(&[("claude", "c"), ("opencode", "b")]));
    let m = default_agent_set_impl(&st, HarnessKind::Claude, None).unwrap();
    assert_eq!(m, map(&[("opencode", "b")]));
    let m = default_agent_set_impl(&st, HarnessKind::Opencode, Some("  ")).unwrap();
    assert!(m.is_empty());
    assert_eq!(on_disk(&dir), Some(BTreeMap::new()));
    // set made the daemon the owner, so a later import is refused.
    assert!(
        !default_agents_import_impl(&st, map(&[("claude", "z")]))
            .unwrap()
            .imported
    );
}

#[test]
fn wholesale_save_without_the_key_keeps_the_map() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(&dir);
    default_agent_set_impl(&st, HarnessKind::Claude, Some("a")).unwrap();
    let payload = SpawnSettings {
        strip_host_env: false,
        ..SpawnSettings::default()
    };
    assert_eq!(payload.default_agents, None);
    spawn_settings_store(&st, payload).unwrap();
    assert!(!st.snapshot().strip_host_env);
    assert_eq!(st.snapshot().default_agents, Some(map(&[("claude", "a")])));
    assert_eq!(on_disk(&dir), Some(map(&[("claude", "a")])));
}

#[test]
fn wholesale_save_with_a_different_map_leaves_the_stored_map() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(&dir);
    default_agent_set_impl(&st, HarnessKind::Claude, Some("a")).unwrap();
    let stale = SpawnSettings {
        default_agents: Some(map(&[("opencode", "old")])),
        ..SpawnSettings::default()
    };
    let saved = spawn_settings_store(&st, stale).unwrap();
    assert_eq!(saved.default_agents, Some(map(&[("claude", "a")])));
    assert_eq!(st.snapshot().default_agents, Some(map(&[("claude", "a")])));
    assert_eq!(on_disk(&dir), Some(map(&[("claude", "a")])));
}

#[test]
fn wholesale_save_with_a_map_while_unset_keeps_it_unset() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(&dir);
    let payload = SpawnSettings {
        default_agents: Some(map(&[("claude", "x")])),
        ..SpawnSettings::default()
    };
    spawn_settings_store(&st, payload).unwrap();
    assert_eq!(st.snapshot().default_agents, None);
    assert_eq!(on_disk(&dir), None);
    let r = default_agents_import_impl(&st, map(&[("claude", "y")])).unwrap();
    assert!(r.imported);
    assert_eq!(r.default_agents, map(&[("claude", "y")]));
}

#[test]
fn import_and_set_refuse_while_degraded_and_leave_the_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = spawn_settings::file_path(dir.path());
    std::fs::write(&path, b"{ not json").unwrap();
    let st = state(&dir);
    *st.degraded.write() = Some("parse error".to_owned());
    let err = default_agents_import_impl(&st, map(&[("claude", "a")])).unwrap_err();
    assert!(err.contains("unreadable"), "{err}");
    let err = default_agent_set_impl(&st, HarnessKind::Claude, Some("a")).unwrap_err();
    assert!(err.contains("unreadable"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    assert_eq!(st.snapshot().default_agents, None);
}
