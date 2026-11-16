//! Port of `mailRetry.test.ts`'s `nextMailRetry`.

use super::*;
use ActivityPhase::{Exited, Idle, Permission, Running, Spawning, Waiting};

fn r(pending: bool, phase: Option<ActivityPhase>, armed: i64, now: i64) -> MailRetry {
    next_mail_retry(pending, phase, armed, now)
}

const WAIT: MailRetry = MailRetry::Wait { delay_ms: 2_000 };

#[test]
fn waits_at_the_interval_when_pending_in_any_live_phase() {
    for p in [Waiting, Spawning, Permission, Running, Idle] {
        assert_eq!(r(true, Some(p), 0, 0), WAIT, "{p:?}");
    }
}

#[test]
fn stops_when_nothing_is_pending_exited_or_no_record() {
    assert_eq!(r(false, Some(Waiting), 0, 0), MailRetry::Stop);
    assert_eq!(r(true, Some(Exited), 0, 0), MailRetry::Stop);
    assert_eq!(r(true, None, 0, 0), MailRetry::Stop);
}

#[test]
fn the_window_boundary_stops_and_just_under_waits() {
    assert_eq!(
        r(true, Some(Waiting), 0, MAIL_RETRY_WINDOW_MS),
        MailRetry::Stop
    );
    assert_eq!(
        r(true, Some(Waiting), 0, MAIL_RETRY_WINDOW_MS + 1),
        MailRetry::Stop
    );
    assert_eq!(r(true, Some(Waiting), 0, MAIL_RETRY_WINDOW_MS - 1), WAIT);
}
