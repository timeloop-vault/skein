//! User input and PTY output. Port of `app/src/harnessActivityIo.ts`; the
//! shared state lives in `machine.rs`.

use super::constants::{INDUCED_MUTE_MS, TAIL_MAX_CHARS};
use super::machine::{PhaseMachine, is_decisive_input, should_arm_watchdog};
use super::patterns::{matches_waiting_prompt, strip_ansi};
use super::types::{ActivityPhase, TransitionSource};

impl PhaseMachine {
    /// Record user input (keystroke / paste) on a harness. `data` is
    /// the exact bytes that keystroke sends to the child (xterm's
    /// `onKey` reports the same string `onData` would carry for it:
    /// real user input, not an auto-response the terminal generates for
    /// a device query).
    ///
    /// Two independent effects: flips `has_user_input` to true exactly
    /// once per spawn (subsequent keystrokes are no-ops there); and, if
    /// the harness is blocked on `permission`, checks whether `data` is
    /// "decisive" (see [`is_decisive_input`]) and if so moves it to
    /// `running`. Claude gives no "the dialog was answered" signal of
    /// its own: the only way the user can answer it is by typing into
    /// the PTY, so that keystroke IS the signal (#86). Doesn't emit for
    /// the `has_user_input` flip alone: consumers that care read the
    /// flag lazily at transition time.
    ///
    /// #259: the first Enter on a harness whose attached adapter has
    /// said nothing yet arms the silent-adapter watchdog (not while the
    /// harness is still `spawning`, #644: that Enter answers a trust dialog).
    /// #644: nor when `empty_composer` says the prompt was empty: a
    /// bare Enter submits nothing and Claude writes no transcript for it.
    /// The caller passes the fact in (`false` = today's behaviour).
    pub fn record_input(&mut self, id: &str, data: &str, empty_composer: bool, now_ms: i64) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        let phase = cur.phase;
        let submitted = data.contains('\r') || data.contains('\n');
        // #423: recorded unconditionally, unlike `prompt_submitted_at` below.
        if submitted {
            cur.last_submit_at = Some(now_ms);
        }
        let arm_watchdog = submitted && !empty_composer && should_arm_watchdog(cur);
        if !cur.has_user_input || arm_watchdog {
            cur.has_user_input = true;
            if arm_watchdog {
                cur.prompt_submitted_at = Some(now_ms);
            }
        }
        // #623: the user answered a screen-set waiting; do not wait for the
        // screen to catch up (two ticks).
        if submitted && self.is_screen_waiting(id) {
            self.clear_screen_ownership(id);
            self.set_phase(
                id,
                ActivityPhase::Running,
                TransitionSource::L2bScreenCleared,
                now_ms,
            );
        }
        if phase == ActivityPhase::Permission && is_decisive_input(data) {
            self.set_phase(
                id,
                ActivityPhase::Running,
                TransitionSource::UserInputPermission,
                now_ms,
            );
        }
    }

    /// #363: arm the #259 silent-adapter watchdog for a prompt submitted
    /// through the `sendPrompt` seam: `create_room`'s first prompt, a
    /// mail nudge, anything that pastes and submits without ever
    /// touching xterm's `onKey` and so never reaches `record_input` above.
    /// Shares `record_input`'s arm guard ([`should_arm_watchdog`], so the
    /// two can't drift apart) minus the "does `data` contain a newline"
    /// check: a `sendPrompt` call IS the submit, unconditionally, the same
    /// as a typed Enter.
    ///
    /// Deliberately narrow: it only arms the timer. Doesn't touch
    /// `has_user_input`: that flag means a human typed something, and a
    /// programmatic submit isn't that. Refuses to arm at all while
    /// `phase == Permission`: an armed timer would eventually strip
    /// authority (`degrade_silent_adapter`) out from under a harness whose
    /// dialog is simply still open, not silent, and never clears
    /// `permission` or moves phase either way: #86 established that only
    /// a decisive user keystroke is proof a dialog was answered, and a
    /// programmatic submit is not one. `canSendPrompt` already refuses to
    /// send into `permission`, but this method doesn't lean on that; it
    /// makes the same call on its own terms, independently, since nothing
    /// stops a future caller from invoking it directly. No emit, matching
    /// `record_input`'s own arm path: `prompt_submitted_at` is bookkeeping
    /// the tick reads, not a phase change any subscriber needs to hear
    /// about.
    pub fn note_prompt_submitted(&mut self, id: &str, now_ms: i64) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        // #423: a seam submit is a submit whatever the watchdog decides.
        cur.last_submit_at = Some(now_ms);
        if cur.phase == ActivityPhase::Permission || !should_arm_watchdog(cur) {
            return;
        }
        cur.prompt_submitted_at = Some(now_ms);
    }

    /// Record a chunk of PTY output. Side-effects: bumps
    /// `last_output_at`; appends the stripped chunk to the L2b tail
    /// buffer (only when no L2c adapter is attached); transitions
    /// `spawning|idle -> running` if applicable; ignored when already
    /// `exited`. Skipped entirely during an active mute window so
    /// induced redraws (focus events, resize) don't reset the idle
    /// timer.
    ///
    /// When a harness has an L2c adapter attached (`authoritative`),
    /// PTY chunks still bump `last_output_at` for diagnostics but do
    /// NOT change phase and do NOT feed the tail buffer. The
    /// adapter is the truth source for those harnesses; running the
    /// pattern matcher on Claude / opencode TUI output would only
    /// invite false positives (an assistant message that quotes
    /// "(y/n)" shouldn't flip the dot).
    ///
    /// `chunk` is optional for backwards compatibility: callers
    /// that don't have the raw bytes (e.g. synthetic recordings)
    /// can pass `None` and the tail buffer stays empty for
    /// that harness.
    pub fn record_output(&mut self, id: &str, chunk: Option<&str>, now_ms: i64) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        if cur.phase == ActivityPhase::Exited {
            return;
        }
        if self.mute_until.get(id).is_some_and(|mute| now_ms < *mute) {
            return;
        }
        if cur.authoritative {
            // Adapter owns phase. Update last_output_at silently so any
            // future detach-fallback to L2a starts with a fresh
            // timestamp; don't touch phase or tail.
            cur.last_output_at = Some(now_ms);
            return;
        }
        // #86: a harness blocked on `permission` stays blocked no
        // matter what repaints across the PTY: the dialog itself is
        // what's producing this output. Only decisive user input
        // (`record_input`) or the adapter's own resolution signal may
        // move it; falling through to the L2b tail/pattern logic below
        // could otherwise misread the dialog's own text as a drained
        // prompt and flip back to running early.
        if cur.phase == ActivityPhase::Permission {
            cur.last_output_at = Some(now_ms);
            return;
        }
        // L2b: append stripped output to the tail buffer. The
        // matcher only looks at the last 256 chars, but we keep
        // 2 KB so a single fat PTY chunk doesn't immediately blow
        // past the matcher's window.
        if let Some(chunk) = chunk.filter(|c| !c.is_empty()) {
            let stripped = strip_ansi(chunk);
            if !stripped.is_empty() {
                cur.tail.push_str(&stripped);
                cap_tail(&mut cur.tail);
            }
        }
        match cur.phase {
            // L2b: if we were in `waiting` and the freshly-arrived
            // output drained the prompt out of the tail (user
            // answered, child kept going), flip back to running.
            // Without this, "Password:" -> user types -> output continues
            // -> we'd stay stuck in waiting because the pattern matched
            // at the moment of transition but not anymore.
            ActivityPhase::Waiting => {
                let still_prompt = matches_waiting_prompt(&cur.tail);
                // #623: a screen-set waiting is the tick's to clear.
                if self.is_screen_waiting(id) || still_prompt {
                    self.set_last_output(id, now_ms);
                    return;
                }
                self.set_phase_with(
                    id,
                    ActivityPhase::Running,
                    TransitionSource::L2bDrained,
                    move |a| a.last_output_at = Some(now_ms),
                    now_ms,
                );
            }
            // Silent mutation: every PTY chunk fires this; emitting
            // would re-render every subscriber on every chunk. The
            // tick reads the latest `last_output_at` so detection
            // stays correct.
            ActivityPhase::Running => cur.last_output_at = Some(now_ms),
            _ => self.set_phase_with(
                id,
                ActivityPhase::Running,
                TransitionSource::PtyOutput,
                move |a| a.last_output_at = Some(now_ms),
                now_ms,
            ),
        }
    }

    fn set_last_output(&mut self, id: &str, now_ms: i64) {
        if let Some(cur) = self.store.get_mut(id) {
            cur.last_output_at = Some(now_ms);
        }
    }

    /// Mute incoming output for this harness for ~800 ms. Call right
    /// before causing an action that's likely to provoke an "induced"
    /// redraw from the child: currently used by `LiveTerminal` when
    /// it forwards a focus-in/-out escape (`\x1b[I` / `\x1b[O`) to
    /// the child. Many TUIs (Claude Code, opencode) redraw their
    /// whole screen in response, which we don't want counted as the
    /// child being "active": the child only redrew because we
    /// poked it.
    pub fn mute_induced_output(&mut self, id: &str, now_ms: i64) {
        self.mute_until
            .insert(id.to_owned(), now_ms + INDUCED_MUTE_MS);
    }
}

/// `combined.slice(-TAIL_MAX_CHARS)`: keep the last `TAIL_MAX_CHARS` chars.
/// Counted in Unicode scalar values; TS counts UTF-16 code units, which
/// differ only for astral characters near the cut.
fn cap_tail(tail: &mut String) {
    let len = tail.chars().count();
    if len > TAIL_MAX_CHARS {
        let cut = tail
            .char_indices()
            .nth(len - TAIL_MAX_CHARS)
            .map_or(0, |(i, _)| i);
        tail.drain(..cut);
    }
}
