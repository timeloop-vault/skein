//! The permission gate (`shellClaim.acceptsPermission`, #318) derived from
//! the stored record, and the Hermes tap (#732).

use std::sync::Arc;

use skein_proto::rooms::ShellClaim;

use super::PhaseTap;
use crate::agent_api::hermes_event::HarnessHermesHook;
use crate::agent_api::state::AgentApiState;
use crate::agent_api::tests::{fixture, harness, room};
use crate::event_sink::EventSink;
use crate::harness_phase::{ActivityPhase, PhaseService};
use crate::room_store::RoomStore;

struct NoEvents;

impl EventSink for NoEvents {
    fn emit_json(&self, _event: &str, _payload: String) -> Result<(), String> {
        Ok(())
    }
}

/// A state whose tap reads one room `r1` with one harness `h1`, which the
/// phase service already knows as just spawned.
struct Fx {
    _f: crate::agent_api::tests::Fixture,
    state: AgentApiState,
    phase: Arc<PhaseService>,
}

fn fx(kind: &str, cmd: &[&str], claim: Option<&str>) -> Fx {
    fx_with(
        kind,
        Some(cmd.iter().map(|s| (*s).to_owned()).collect()),
        claim,
    )
}

fn fx_with(kind: &str, cmd: Option<Vec<String>>, claim: Option<&str>) -> Fx {
    let f = fixture();
    let mut h = harness("h1", kind, "h1");
    h.cmd = cmd;
    h.shell_claim = claim.map(|s| ShellClaim {
        session_id: s.to_owned(),
        port: None,
    });
    let rooms = RoomStore::new(Arc::clone(&f.db), Arc::new(NoEvents));
    rooms.fill(vec![room("r1", vec![h])]).unwrap();
    let phase = PhaseService::new();
    phase.spawned("h1", "r1", true);
    let mut state = AgentApiState::for_test(Arc::clone(&f.db));
    state.phase_tap = Some(PhaseTap {
        service: Arc::clone(&phase),
        rooms,
    });
    Fx {
        _f: f,
        state,
        phase,
    }
}

impl Fx {
    fn ping(&self, session_id: Option<&str>) -> ActivityPhase {
        self.state
            .phase_permission("r1", "h1", Some("Bash"), None, None, session_id);
        self.phase.get("h1").unwrap().phase
    }
}

#[test]
fn outside_shell_mode_a_ping_is_always_accepted() {
    // Running its own program: any session id, even a foreign one.
    let x = fx("claude", &["claude", "--resume", "s1"], None);
    assert_eq!(x.ping(Some("someone-else")), ActivityPhase::Permission);
    // A path and a Windows extension still read as the program.
    let x = fx("claude", &["C:\\bin\\Claude.CMD"], None);
    assert_eq!(x.ping(None), ActivityPhase::Permission);
    // Not a Claude harness: no shell mode to speak of.
    let x = fx("opencode", &["bash"], None);
    assert_eq!(x.ping(None), ActivityPhase::Permission);
}

#[test]
fn a_shell_without_a_claim_drops_the_ping() {
    let x = fx("claude", &["bash"], None);
    assert_eq!(x.ping(None), ActivityPhase::Spawning);
    assert_eq!(x.ping(Some("s1")), ActivityPhase::Spawning);
}

/// `shellClaim.noteSpawn`: a missing argv0 is "not running the kind's
/// program", i.e. shell mode, exactly as an explicit shell is.
#[test]
fn a_claude_record_with_no_cmd_is_in_shell_mode() {
    let x = fx_with("claude", None, None);
    assert_eq!(x.ping(Some("s1")), ActivityPhase::Spawning);
    let x = fx_with("claude", Some(Vec::new()), None);
    assert_eq!(x.ping(None), ActivityPhase::Spawning);
    let x = fx_with("claude", None, Some("s1"));
    assert_eq!(x.ping(Some("s1")), ActivityPhase::Permission);
    // Other kinds have no shell mode, cmd or not.
    let x = fx_with("opencode", None, None);
    assert_eq!(x.ping(None), ActivityPhase::Permission);
}

#[test]
fn a_claimed_shell_accepts_the_matching_or_a_missing_session() {
    let x = fx("claude", &["/bin/zsh"], Some("s1"));
    assert_eq!(x.ping(Some("s1")), ActivityPhase::Permission);
    let x = fx("claude", &["/bin/zsh"], Some("s1"));
    assert_eq!(x.ping(None), ActivityPhase::Permission);
    let x = fx("claude", &["/bin/zsh"], Some("s1"));
    assert_eq!(x.ping(Some("")), ActivityPhase::Permission);
}

#[test]
fn a_claimed_shell_drops_a_different_session() {
    let x = fx("claude", &["/bin/zsh"], Some("s1"));
    assert_eq!(x.ping(Some("nested-child")), ActivityPhase::Spawning);
}

#[test]
fn an_unknown_harness_is_accepted_like_an_unknown_shell_state() {
    let x = fx("claude", &["bash"], None);
    x.state
        .phase_permission("r1", "no-such", None, None, None, None);
    x.state
        .phase_permission("no-such-room", "h1", None, None, None, None);
    assert_eq!(x.phase.get("h1").unwrap().phase, ActivityPhase::Permission);
}

#[test]
fn a_state_without_a_tap_ignores_the_hooks() {
    let f = fixture();
    let state = AgentApiState::for_test(Arc::clone(&f.db));
    state.phase_permission("r1", "h1", None, None, None, None);
    state.phase_hermes_hook(&HarnessHermesHook::default());
}

#[test]
fn a_hermes_hook_reaches_the_service_without_failing_the_route() {
    let x = fx("hermes", &["hermes"], None);
    x.state.phase_hermes_hook(&HarnessHermesHook {
        room_id: "r1".to_owned(),
        harness_id: "h1".to_owned(),
        event: "pre_llm_call".to_owned(),
        session_id: Some("hs".to_owned()),
        ..HarnessHermesHook::default()
    });
    // The harness is still tracked and the call did not panic; the phase
    // semantics of each hook are the translator's own tests.
    assert!(x.phase.get("h1").is_some());
}
