//! The resolver and the record builder, without a daemon or a CLI.

use std::collections::BTreeMap;

use super::*;
use crate::harness_argv::AgentVerdict;

fn defaults(kind: &str, agent: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(kind.to_owned(), agent.to_owned())])
}

fn never(_: &str) -> AgentVerdict {
    panic!("no agent should have been checked");
}

#[test]
fn kind_defaults_to_claude_and_accepts_any_other_known_kind() {
    assert_eq!(resolve_kind(None).unwrap(), HarnessKind::Claude);
    assert_eq!(
        resolve_kind(Some("opencode")).unwrap(),
        HarnessKind::Opencode
    );
    assert_eq!(resolve_kind(Some("files")).unwrap(), HarnessKind::Files);
}

#[test]
fn an_unknown_kind_and_remote_are_refused_with_their_messages() {
    let err = resolve_kind(Some("bogus")).unwrap_err();
    assert!(matches!(&err, VerbError::Refused(m) if m == "unknown harness kind \"bogus\""));
    let err = resolve_kind(Some("remote")).unwrap_err();
    assert!(matches!(&err, VerbError::Refused(m) if m.contains("only the user can enter")));
}

#[test]
fn a_requested_agent_wins_over_the_default() {
    let d = defaults("claude", "fromsettings");
    let got = resolve_agent_with(Some("asked"), HarnessKind::Claude, Some(&d), |name| {
        assert_eq!(name, "asked");
        AgentVerdict::Ok
    });
    assert_eq!(got.unwrap().as_deref(), Some("asked"));
}

#[test]
fn the_default_agent_applies_when_none_is_requested() {
    let d = defaults("claude", "fromsettings");
    let got = resolve_agent_with(None, HarnessKind::Claude, Some(&d), |name| {
        assert_eq!(name, "fromsettings");
        AgentVerdict::Ok
    });
    assert_eq!(got.unwrap().as_deref(), Some("fromsettings"));
    // A default for another kind is not this kind's.
    let got = resolve_agent_with(None, HarnessKind::Opencode, Some(&d), never);
    assert_eq!(got.unwrap(), None);
}

#[test]
fn an_explicit_empty_agent_means_none_and_beats_the_default() {
    let d = defaults("claude", "fromsettings");
    let got = resolve_agent_with(Some(""), HarnessKind::Claude, Some(&d), never);
    assert_eq!(got.unwrap(), None);
    let got = resolve_agent_with(Some("  "), HarnessKind::Claude, None, never);
    assert_eq!(got.unwrap(), None);
}

#[test]
fn a_kind_that_takes_no_agent_gets_none_and_checks_nothing() {
    let d = defaults("byoh", "x");
    for kind in [HarnessKind::Byoh, HarnessKind::Copilot, HarnessKind::Files] {
        let got = resolve_agent_with(Some("x"), kind, Some(&d), never);
        assert_eq!(got.unwrap(), None, "{kind}");
    }
}

#[test]
fn only_the_unknown_verdict_refuses() {
    let err = resolve_agent_with(Some("gone"), HarnessKind::Claude, None, |a| {
        AgentVerdict::Unknown {
            agent: a.to_owned(),
        }
    })
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m)
            if m.starts_with("agent \"gone\" is not one Claude Code offers any more.")
                && m.contains("a resumed harness")),
        "{err:?}"
    );
    let err = resolve_agent_with(Some("gone"), HarnessKind::Opencode, None, |a| {
        AgentVerdict::Unknown {
            agent: a.to_owned(),
        }
    })
    .unwrap_err();
    assert!(matches!(&err, VerbError::Refused(m) if m.contains("opencode offers")));

    let got = resolve_agent_with(Some("maybe"), HarnessKind::Claude, None, |a| {
        AgentVerdict::Unverified {
            agent: a.to_owned(),
            why: "cli down".to_owned(),
        }
    });
    assert_eq!(got.unwrap().as_deref(), Some("maybe"));
}

#[test]
fn names_count_the_room_harnesses_and_use_the_tab_labels() {
    assert_eq!(harness_display_name(HarnessKind::Claude, 0), "CC-1");
    assert_eq!(harness_display_name(HarnessKind::Opencode, 2), "oc-3");
    assert_eq!(harness_display_name(HarnessKind::Files, 1), "files-2");
    assert_eq!(harness_display_name(HarnessKind::Byoh, 0), "sh-1");
}

#[test]
fn random_ids_are_h_and_five_base36_characters() {
    for _ in 0..50 {
        let id = random_id();
        assert_eq!(id.len(), 6, "{id}");
        assert!(id.starts_with('h'));
        assert!(
            id[1..]
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
            "{id}"
        );
    }
}

fn spec(kind: HarnessKind, agent: Option<&str>) -> NewHarness {
    NewHarness {
        kind,
        agent: agent.map(str::to_owned),
        name: "n".to_owned(),
        cwd: Some("/work".to_owned()),
        created_by: Some(HarnessCreatedBy {
            room_id: "r".to_owned(),
            harness_id: Some("h".to_owned()),
        }),
    }
}

fn fresh() -> FreshCmdOutput {
    FreshCmdOutput {
        cmd: vec![
            "claude".to_owned(),
            "--session-id".to_owned(),
            "s".to_owned(),
        ],
        session_id: Some("s".to_owned()),
        hermes_profile: None,
    }
}

#[test]
fn a_pty_record_is_running_live_and_carries_the_fresh_argv() {
    let h = assemble(
        "hx".to_owned(),
        spec(HarnessKind::Claude, Some("rev")),
        Some(fresh()),
    );
    assert_eq!(h.id, "hx");
    assert_eq!(h.kind, "claude");
    assert_eq!(h.name, "n");
    assert_eq!(h.status, "running");
    assert_eq!(h.model, "sonnet-4.5");
    assert_eq!(h.tokens, "0");
    assert_eq!(h.live, Some(true));
    assert_eq!(h.cmd, Some(fresh().cmd));
    assert_eq!(h.session_id.as_deref(), Some("s"));
    assert_eq!(h.agent.as_deref(), Some("rev"));
    assert_eq!(h.cwd.as_deref(), Some("/work"));
    assert_eq!(h.created_by.as_ref().unwrap().room_id, "r");
    assert!(h.launch.is_none() && h.opencode_version.is_none());
}

#[test]
fn a_copilot_record_has_its_own_model() {
    let h = assemble(
        "hx".to_owned(),
        spec(HarnessKind::Copilot, None),
        Some(fresh()),
    );
    assert_eq!(h.model, "gpt-5");
}

#[test]
fn a_surface_record_is_idle_with_no_process_fields() {
    let h = assemble("hx".to_owned(), spec(HarnessKind::Files, None), None);
    assert_eq!(h.status, "idle");
    assert_eq!(h.model, "");
    assert_eq!(h.live, None);
    assert_eq!(h.cmd, None);
    assert_eq!(h.session_id, None);
}

#[test]
fn a_blank_agent_is_not_stored() {
    let h = assemble("hx".to_owned(), spec(HarnessKind::Claude, Some(" ")), None);
    assert_eq!(h.agent, None);
}

// ── the locked final step of open_harness ───────────────────────────────

mod store_step {
    use super::super::*;
    use super::spec;
    use crate::agent_api::tests::{agent_api_state_with_room_records, fixture, harness, room};
    use skein_proto::room_mutations::RoomMutation;

    fn stored_harnesses(store: &RoomStore, id: &str) -> Vec<Harness> {
        store
            .rooms()
            .into_iter()
            .find(|r| r.id == id)
            .unwrap()
            .harnesses
    }

    #[tokio::test]
    async fn a_room_archived_after_the_guards_gets_no_harness() {
        let f = fixture();
        let (state, store) = agent_api_state_with_room_records(
            &f,
            vec![room("r1", vec![harness("h1", "claude", "main")])],
        );
        let built = assemble("hn".to_owned(), spec(HarnessKind::Files, None), None);
        // The archive lands between the guards and the put.
        let mut archived = store.rooms().into_iter().next().unwrap();
        archived.archived = Some(5);
        store
            .apply("client", vec![RoomMutation::UpdateRoom { room: archived }])
            .unwrap();
        let err = store_harness(&state, "r1", built, 8).await.unwrap_err();
        assert!(
            matches!(&err, VerbError::Refused(m) if m.starts_with("archived:")),
            "{err:?}"
        );
        assert_eq!(stored_harnesses(&store, "r1").len(), 1);
    }

    #[tokio::test]
    async fn the_cap_is_rechecked_against_the_fresh_count() {
        let f = fixture();
        let (state, store) = agent_api_state_with_room_records(
            &f,
            vec![room(
                "r1",
                (0..3)
                    .map(|i| harness(&format!("h{i}"), "claude", &format!("n{i}")))
                    .collect(),
            )],
        );
        let built = assemble("hn".to_owned(), spec(HarnessKind::Files, None), None);
        let err = store_harness(&state, "r1", built, 3).await.unwrap_err();
        assert!(
            matches!(&err, VerbError::Refused(m) if m.starts_with("room_full:")),
            "{err:?}"
        );
        assert_eq!(stored_harnesses(&store, "r1").len(), 3);
    }

    #[tokio::test]
    async fn the_name_comes_from_the_fresh_count_and_skips_a_taken_one() {
        let f = fixture();
        // `files-2` is taken while only one other harness exists, as after a
        // close: the count alone would name the new one `files-2` again.
        let (state, store) = agent_api_state_with_room_records(
            &f,
            vec![room(
                "r1",
                vec![
                    harness("h1", "claude", "main"),
                    harness("h2", "files", "files-3"),
                ],
            )],
        );
        let built = assemble("hn".to_owned(), spec(HarnessKind::Files, None), None);
        let stored = store_harness(&state, "r1", built, 8).await.unwrap();
        assert_eq!(stored.name, "files-4");
        assert_eq!(stored_harnesses(&store, "r1").len(), 3);
    }
}
