// submitRetry — the #380 gap-then-retry policy for a prompt delivered
// through the #238 seam as the FIRST thing into a freshly spawned
// Claude Code harness. Claude Code 2.1.283 sometimes drops the Enter
// that ends a bracketed paste when it lands in the same stdin chunk as
// the paste's own end — a startup-timing race, not something Skein
// controls: upstream anthropics/claude-code#91205 describes the same
// symptom (a key arriving in the same stdin chunk as the end of a
// bracketed paste being mishandled). Later deliveries into an already
// running session never showed it, which is consistent with a
// startup-only race rather than a general Enter-eating bug.
//
// `sendPrompt` (harnessInput.ts) works around it two ways: it no
// longer writes the submit "\r" in the same tick as the paste —
// `SUBMIT_GAP_MS` gives the CLI a beat to finish rendering the pasted
// text before the Enter reaches its own stdin read — and, for kinds
// that opt in (`capabilities.submitRetry`, Claude only), it watches
// for `SUBMIT_RETRY_AFTER_MS` after the submit and fires exactly one
// more "\r" if nothing proves the first one landed.
// `decideSubmitRetry` is the pure decision underneath that watch — no
// DOM, no store, no timers — so the policy is testable without a
// harness or a terminal.
//
// A missed retry window is safer than a spurious one: an extra "\r"
// into a composer that already sent its text is, per Claude Code's own
// known behaviour, a no-op — submitting an empty prompt is ignored
// (stated from that behaviour, not re-verified live in this change) —
// so this stays a blunt one-shot retry rather than something that has
// to prove the first Enter was actually lost.

import type { ActivityPhase } from "./harnessActivityTypes.ts";

/// How long `sendPrompt` waits after pasting before writing the
/// submit "\r" — long enough for a freshly spawned Claude Code to
/// finish rendering the paste before the Enter reaches its stdin,
/// short enough that nobody watching the terminal notices a pasted
/// prompt sitting unsent for a moment.
export const SUBMIT_GAP_MS = 150;

/// How long `sendPrompt` watches a submitted, retry-capable harness
/// before deciding the first "\r" didn't land and sending one more.
/// Well inside `ADAPTER_SILENT_AFTER_MS` (#259, 10 s) — the retry
/// decision has to be made, one way or the other, before that watchdog
/// would degrade the adapter for a submit it thinks went unanswered.
export const SUBMIT_RETRY_AFTER_MS = 3_000;

/// What `decideSubmitRetry` needs to make the call — gathered by the
/// caller so this stays pure and DOM-free, testable with no store
/// singleton and no xterm instance.
export interface DecideSubmitRetryInput {
	/// Does this harness kind opt into the retry at all
	/// (`capabilities.submitRetry`, #380) — false for every kind but
	/// Claude, so opencode's behaviour is untouched.
	capable: boolean;
	/// Did any phase transition *out of* `waiting` land for this
	/// harness between the submit and now? That's proof the Enter was
	/// received — the CLI doesn't leave `waiting` on its own — so a
	/// harness that ever left is never retried, even if it's back in
	/// `waiting` again by the time the window closes.
	leftWaitingSinceSend: boolean;
	/// The harness's phase right now, or `null` if it has no activity
	/// record at all.
	phase: ActivityPhase | null;
	/// Did the user type anything into this harness since the submit?
	/// A human keystroke means a machine-written Enter might land on
	/// top of something the user typed, not the pasted prompt.
	userInputSinceSend: boolean;
	/// Is this still the same seam registration that was live at
	/// submit time? A respawn since then means the retry would write
	/// into a different terminal than the one the paste went to.
	sameTarget: boolean;
}

export type SubmitRetryDecision = { retry: true } | { retry: false; reason: string };

/// Pure policy: retry only when every one of these holds —
///
///  - the kind opted in (`capable`);
///  - the harness never left `waiting` since the submit
///    (`!leftWaitingSinceSend`) — it already proved the first Enter
///    landed, so there's nothing left to fix;
///  - the registration hasn't changed (`sameTarget`) — the terminal a
///    retry would write into has to be the one the paste went to;
///  - the user hasn't typed since the submit (`!userInputSinceSend`)
///    — a human is driving now, and machine-written input isn't safe
///    to layer on top of whatever they typed;
///  - and its phase is `waiting` right now — explicitly NOT
///    `permission`: a dialog opening after the submit is itself proof
///    the Enter was seen (it's what triggered the dialog), never that
///    it was dropped, and answering a dialog is #86's territory, not
///    this one's. Every other phase (`running`, `idle`, `spawning`,
///    `exited`, or no activity record at all) is refused the same way
///    — there's nothing left to submit into.
export function decideSubmitRetry(input: DecideSubmitRetryInput): SubmitRetryDecision {
	if (!input.capable) {
		return { retry: false, reason: "this harness kind doesn't opt into a submit retry" };
	}
	if (input.leftWaitingSinceSend) {
		return {
			retry: false,
			reason: "the harness left waiting since the submit — the first Enter landed",
		};
	}
	if (!input.sameTarget) {
		return { retry: false, reason: "the terminal registration changed since the submit" };
	}
	if (input.userInputSinceSend) {
		return { retry: false, reason: "the user typed into the harness since the submit" };
	}
	if (input.phase !== "waiting") {
		return {
			retry: false,
			reason: `the harness isn't waiting any more (currently ${input.phase ?? "no activity record"})`,
		};
	}
	return { retry: true };
}
