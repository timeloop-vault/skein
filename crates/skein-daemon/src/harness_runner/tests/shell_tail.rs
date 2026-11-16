//! A Claude harness in post-exit-shell mode runs a shell, not Claude, so the
//! daemon must not tail the old transcript: replaying it settled the phase
//! to `waiting` (#763).

use skein_proto::rooms::ShellClaim;

use super::*;
use crate::harness_phase::ActivityPhase;

const SID: &str = "7f3c0a52-0000-4000-8000-000000000763";

fn phase_of(fx: &Fx) -> Option<ActivityPhase> {
    fx.daemon.phase.get("h1").map(|a| a.phase)
}

/// A transcript of `SID` that ended its turn: tailed, it would read `waiting`.
fn seed_finished_transcript(fx: &Fx) {
    let work = fx.work.to_string_lossy().into_owned();
    let path = skein_harness::claude::session_jsonl_path(&fx.home, &work, SID);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        concat!(
            r#"{"type":"user","uuid":"u1","timestamp":"2026-05-15T21:16:20.000Z","message":{"role":"user","content":"hi"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
}

fn shell_fx(claim: Option<ShellClaim>) -> Fx {
    let fx = Fx::new();
    seed_finished_transcript(&fx);
    fx.runner().set_session_probe(Arc::new(|_| Ok(true)));
    let mut h = in_shell(harness("h1", "claude", Some(shell_cmd()), Some(SID)));
    h.shell_claim = claim;
    fx.create(room("r1", &fx.work, vec![h]));
    fx
}

fn claim() -> ShellClaim {
    ShellClaim {
        session_id: SID.to_owned(),
        port: None,
    }
}

#[test]
fn a_shell_launch_without_a_claim_gets_no_claude_tail() {
    let fx = shell_fx(None);
    fx.ensure(false, None).unwrap();
    assert!(!fx.daemon.claude.is_attached("h1"));
    std::thread::sleep(Duration::from_millis(500));
    assert_ne!(phase_of(&fx), Some(ActivityPhase::Waiting));

    // A view re-attaching later does not tail it either.
    fx.runner().release_events("h1");
    assert!(!fx.daemon.claude.is_attached("h1"));
}

#[test]
fn restart_harness_leaves_the_shell_and_tails_claude_again() {
    let fx = shell_fx(None);
    fx.ensure(false, None).unwrap();
    assert!(!fx.daemon.claude.is_attached("h1"));

    fx.runner().set_stand_in(shell_cmd());
    let mut tool = harness("h1", "claude", Some(vec!["claude".to_owned()]), Some(SID));
    tool.launch = None;
    fx.apply(RoomMutation::PutHarness {
        room_id: "r1".to_owned(),
        harness: tool,
    });
    fx.ensure(true, None).unwrap();
    assert!(fx.daemon.claude.is_attached("h1"));
}

#[test]
fn a_shell_claim_means_the_tool_runs_and_is_tailed() {
    let fx = shell_fx(Some(claim()));
    fx.ensure(false, None).unwrap();
    assert!(fx.daemon.claude.is_attached("h1"));
}
