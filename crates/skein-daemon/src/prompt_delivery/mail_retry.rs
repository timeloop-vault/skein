//! The #386 bounded retry decision: a port of `nextMailRetry`
//! (`app/src/mailRetry.ts`). Pure; the caller owns the timer.

use crate::harness_phase::ActivityPhase;

/// How often a retry tick re-runs the check while mail is pending.
pub const MAIL_RETRY_INTERVAL_MS: i64 = 2_000;
/// How long the retry keeps ticking after it was last armed.
pub const MAIL_RETRY_WINDOW_MS: i64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailRetry {
    Stop,
    Wait { delay_ms: i64 },
}

/// Stops when nothing is pending, the harness is gone (`exited` or no
/// activity record), or the window elapsed (`>=`); otherwise waits one
/// interval, whatever the phase.
#[must_use]
pub fn next_mail_retry(
    pending: bool,
    phase: Option<ActivityPhase>,
    armed_at_ms: i64,
    now_ms: i64,
) -> MailRetry {
    if !pending || phase.is_none() || phase == Some(ActivityPhase::Exited) {
        return MailRetry::Stop;
    }
    if now_ms - armed_at_ms >= MAIL_RETRY_WINDOW_MS {
        return MailRetry::Stop;
    }
    MailRetry::Wait {
        delay_ms: MAIL_RETRY_INTERVAL_MS,
    }
}

#[cfg(test)]
mod tests;
