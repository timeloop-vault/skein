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
// that opt in (`capabilities.submitRetry`, Claude only), it follows up
// with as many as three more "\r"s, at `SUBMIT_RETRY_SCHEDULE_MS`,
// stopping the moment any one of them proves unnecessary or unsafe.
// `decideSubmitRetry` is the pure decision underneath each of those
// checks — no DOM, no store, no timers — so the policy is testable
// without a harness or a terminal.
//
// The schedule is bounded by, and stays inside, the #259 silent-adapter
// watchdog's own `ADAPTER_SILENT_AFTER_MS` window (harnessActivity.ts /
// harnessActivityCore.ts's `degradeSilentAdapter`) — every entry here
// is well under it. That isn't just tidy timing: once the watchdog
// fires it sets `adapterSilent: true` and `authoritative: false`, and
// from that moment "the harness left `waiting`" is no longer
// authoritative proof of anything, since nothing is confirmed to still
// be watching it. Retrying past that point would be exactly the kind
// of send `canSendPrompt` refuses everywhere else — "no nudge where
// safety can't be proven" — so `decideSubmitRetry`'s `watched` check
// refuses the instant the watchdog has degraded the adapter, rather
// than let the schedule run to its end blind. Three attempts, not one,
// because an unverified report says Claude may sit on a dropped Enter
// longer than a single quick recheck covers; three checks spread across
// the watchdog's window costs little and covers more of that
// uncertainty than one does.
//
// A missed retry window is safer than a spurious one: an extra "\r"
// into a composer that already sent its text is, per Claude Code's own
// known behaviour, a no-op — submitting an empty prompt is ignored
// (stated from that behaviour, not re-verified live in this change) —
// so each retry stays a blunt attempt rather than something that has
// to prove the previous one was actually lost.
//
// #381: the "stopping point" a submit landed at isn't only `waiting` any
// more — a #277 delegation deferral (`running` with a non-null
// `delegationDeferredAt`) is an equally safe one, since it means the
// main session already ended its turn and only stayed `running` because
// background subagents were still working (see `harnessActivityCore.ts`'s
// `atSafeStoppingPoint`). `leftWaitingSinceSend` and a bare `phase ===
// "waiting"` check can't tell that arm apart from an ordinary mid-turn
// `running`, so this module now also takes `deferredAtSend` (the arm
// `sendPrompt` snapshotted at submit time — `null` for a plain-`waiting`
// send) and `deferredAtNow` (the harness's current `delegationDeferredAt`
// at decision time). The conservative rule: any change to the armed
// value since send — disarmed by real work, flushed by the tick's
// settle/ceiling to `waiting`, or a fresh re-arm with a different
// timestamp — reads exactly like leaving `waiting` did before, and ends
// the retry sequence with no further "\r".

import type { ActivityPhase } from "./harnessActivityTypes.ts";

/// How long `sendPrompt` waits after pasting before writing the
/// submit "\r" — long enough for a freshly spawned Claude Code to
/// finish rendering the paste before the Enter reaches its stdin,
/// short enough that nobody watching the terminal notices a pasted
/// prompt sitting unsent for a moment.
export const SUBMIT_GAP_MS = 150;

/// Delays, measured from the first submit, at which `sendPrompt`
/// rechecks a retry-capable harness and fires one more "\r" if nothing
/// proves the previous one landed. Every entry is well under
/// `ADAPTER_SILENT_AFTER_MS` (#259, 10 s in `harnessActivityConstants.ts`)
/// — see this file's header for why the schedule stops there rather
/// than running longer.
export const SUBMIT_RETRY_SCHEDULE_MS = [2_000, 4_500, 8_000] as const;

/// What `decideSubmitRetry` needs to make the call — gathered by the
/// caller so this stays pure and DOM-free, testable with no store
/// singleton and no xterm instance.
export interface DecideSubmitRetryInput {
	/// Does this harness kind opt into the retry at all
	/// (`capabilities.submitRetry`, #380) — false for every kind but
	/// Claude, so opencode's behaviour is untouched.
	capable: boolean;
	/// Is anything still confirmed to be watching this harness —
	/// `activity.authoritative && !activity.adapterSilent`, computed by
	/// the caller? Once the #259 watchdog has degraded the adapter,
	/// `leftStoppingPointSinceSend` staying false is no longer
	/// authoritative proof the Enter didn't land — it might just mean
	/// nothing is listening any more — so a retry past that point would
	/// be exactly the kind of blind send `canSendPrompt` refuses
	/// everywhere else.
	watched: boolean;
	/// Did any phase transition *out of* `waiting` land for this
	/// harness between the submit and now? That's proof the Enter was
	/// received — the CLI doesn't leave `waiting` on its own — so a
	/// harness that ever left is never retried, even if it's back in
	/// `waiting` again by the time a later attempt runs. Only ever set
	/// for the plain-`waiting` arm: a delegation-deferred arm has no such
	/// transition to fire, since disarming it doesn't change the phase
	/// (see `deferredAtSend`/`deferredAtNow` below for that arm's
	/// equivalent proof).
	leftStoppingPointSinceSend: boolean;
	/// The harness's phase right now, or `null` if it has no activity
	/// record at all.
	phase: ActivityPhase | null;
	/// #381: which #277 delegation deferral (if any) this submit was
	/// made under — the caller's `delegationDeferredAt` snapshot taken
	/// at submit time, or `null` for a submit made at a plain `waiting`.
	/// Paired with `deferredAtNow` below.
	deferredAtSend: number | null;
	/// #381: the harness's current `delegationDeferredAt`, sampled by the
	/// caller fresh at decision time. Retrying requires this to still
	/// equal `deferredAtSend` — a disarm (work landed), a flush to
	/// `waiting` (the tick's settle/ceiling), or a fresh re-arm all
	/// change this value and each ends the sequence, the same way
	/// `leftStoppingPointSinceSend` ends it for the plain-`waiting` arm.
	deferredAtNow: number | null;
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
///  - something is still confirmed to be watching (`watched`) — once
///    the #259 watchdog has degraded the adapter, nothing below this
///    check can be trusted as proof of anything;
///  - the harness never left `waiting` since the submit
///    (`!leftStoppingPointSinceSend`) — it already proved the Enter
///    landed, so there's nothing left to fix. Only ever true for the
///    plain-`waiting` arm — see the stopping-point check below for the
///    delegation-deferred arm's own equivalent;
///  - the registration hasn't changed (`sameTarget`) — the terminal a
///    retry would write into has to be the one the paste went to;
///  - the user hasn't typed since the submit (`!userInputSinceSend`)
///    — a human is driving now, and machine-written input isn't safe
///    to layer on top of whatever they typed;
///  - and the harness is still at the SAME stopping point right now
///    (#381) — either plain `waiting` (`phase === "waiting" &&
///    deferredAtSend === null`), or the identical #277 delegation
///    deferral it was submitted under (`phase === "running" &&
///    deferredAtSend !== null && deferredAtNow === deferredAtSend`).
///    Explicitly NOT `permission` in either case: a dialog opening
///    after the submit is itself proof the Enter was seen (it's what
///    triggered the dialog), never that it was dropped, and answering a
///    dialog is #86's territory, not this one's. Every other case —
///    `running`/`idle`/`spawning`/`exited` with no matching deferral, a
///    disarm, a flush to `waiting`, a fresh re-arm with a different
///    timestamp, or no activity record at all — is refused the same
///    way: there's nothing left to submit into, or it isn't the same
///    stopping point this submit was made for.
export function decideSubmitRetry(input: DecideSubmitRetryInput): SubmitRetryDecision {
	if (!input.capable) {
		return { retry: false, reason: "this harness kind doesn't opt into a submit retry" };
	}
	if (!input.watched) {
		return {
			retry: false,
			reason: "no confirmed adapter is watching this harness any more — retrying blind isn't safe",
		};
	}
	if (input.leftStoppingPointSinceSend) {
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
	const stillAtStoppingPoint =
		(input.phase === "waiting" && input.deferredAtSend === null) ||
		(input.phase === "running" &&
			input.deferredAtSend !== null &&
			input.deferredAtNow === input.deferredAtSend);
	if (!stillAtStoppingPoint) {
		return {
			retry: false,
			reason: `the harness isn't at its stopping point any more (currently ${input.phase ?? "no activity record"})`,
		};
	}
	return { retry: true };
}
