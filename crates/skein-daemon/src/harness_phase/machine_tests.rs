//! Smoke tests for the wiring; the full tables are ported separately.

use super::constants::IDLE_AFTER_MS;
use super::patterns::{matches_waiting_prompt, strip_ansi};
use super::{ActivityPhase, PhaseMachine, TransitionSource};

#[test]
fn spawned_then_output_then_idle_via_tick() {
    let mut m = PhaseMachine::new();
    m.spawned("h", 0);
    assert_eq!(m.get("h").unwrap().phase, ActivityPhase::Spawning);
    m.record_output("h", Some("hello"), 100);
    assert_eq!(m.get("h").unwrap().phase, ActivityPhase::Running);
    m.tick(100 + IDLE_AFTER_MS - 1);
    assert_eq!(m.get("h").unwrap().phase, ActivityPhase::Running);
    m.tick(100 + IDLE_AFTER_MS);
    assert_eq!(m.get("h").unwrap().phase, ActivityPhase::Idle);

    let t = m.drain_transitions();
    let seen: Vec<_> = t.iter().map(|t| (t.from, t.to, t.source)).collect();
    assert_eq!(
        seen,
        vec![
            (
                ActivityPhase::Spawning,
                ActivityPhase::Running,
                TransitionSource::PtyOutput
            ),
            (
                ActivityPhase::Running,
                ActivityPhase::Idle,
                TransitionSource::L2aIdle
            ),
        ]
    );
    assert_eq!(m.drain_changed(), vec!["h".to_owned()]);
    assert_eq!(m.drain_transitions().len(), 0);
}

#[test]
fn exited_is_idempotent_and_records_code() {
    let mut m = PhaseMachine::new();
    m.spawned("h", 0);
    m.exited("h", Some(3), 10);
    m.exited("h", Some(4), 20);
    let a = m.get("h").unwrap();
    assert_eq!((a.phase, a.exit_code), (ActivityPhase::Exited, Some(3)));
    assert_eq!(m.drain_transitions().len(), 1);
    m.forget("h");
    assert!(m.get("h").is_none());
}

#[test]
fn permission_cleared_by_decisive_input_and_resets_fields() {
    let mut m = PhaseMachine::new();
    m.spawned("h", 0);
    m.set_permission_from_adapter(
        "h",
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        Some("a1"),
        5,
    );
    assert_eq!(m.get("h").unwrap().permission_tool.as_deref(), Some("Bash"));
    m.record_input("h", "\x1b[A", false, 6);
    assert_eq!(m.get("h").unwrap().phase, ActivityPhase::Permission);
    m.record_input("h", "y", false, 7);
    let a = m.get("h").unwrap();
    assert_eq!(a.phase, ActivityPhase::Running);
    assert_eq!(
        (a.permission_tool.as_deref(), a.permission_at),
        (None, None)
    );
}

#[test]
fn sources_serialize_to_ts_strings() {
    assert_eq!(
        TransitionSource::L2c1ClaudePermission.as_str(),
        "l2c1-claude-permission-request"
    );
    assert_eq!(
        serde_json::to_string(&TransitionSource::DelegationCeiling).unwrap(),
        "\"delegation-ceiling\""
    );
    assert_eq!(
        serde_json::to_string(&ActivityPhase::Waiting).unwrap(),
        "\"waiting\""
    );
}

#[test]
fn waiting_patterns_and_ansi() {
    assert!(matches_waiting_prompt("Do it? (y/n) "));
    assert!(matches_waiting_prompt("[sudo] password for bob: "));
    assert!(matches_waiting_prompt("Continue? [y/N]"));
    assert!(matches_waiting_prompt("Press ANY key to go on"));
    assert!(!matches_waiting_prompt("user@host:~$ "));
    assert!(!matches_waiting_prompt("password: hunter2\nok"));
    assert_eq!(
        strip_ansi("\x1b[31mred\x1b[0m\x1b]0;title\x07\x07x"),
        "redx"
    );
}
