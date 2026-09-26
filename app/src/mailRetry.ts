// #386: pending agent mail must be re-evaluated against the UNCHANGED
// send gate at every safe moment, not only on the one transition into
// `waiting` — the bug that motivated this: mail lands while a fresh
// Claude harness is still `spawning` (the gate correctly refuses), the
// one `spawning → waiting` check either never fires or is itself
// refused (the #238 seam hasn't registered yet), and nothing ever
// re-checks again. This module is pure and DOM-free — no store, no
// timers of its own — `useMailDelivery.ts` owns the actual
// `setTimeout` and reads `harnessActivity`/`harnessInput` state to
// build the inputs below.
//
// The bounded retry this module drives never loosens `canSendPrompt`
// or `decideMailNudge` — it only asks them again, more often, for as
// long as mail is known to be outstanding. Dedupe (never nudging twice
// for the same unread count) stays entirely with `decideMailNudge`'s
// own `lastNudged` bookkeeping; this module has no opinion on whether
// a nudge actually goes out, only on whether it's worth checking again
// soon.

import type { ActivityPhase } from "./harnessActivityTypes.ts";

/// How often a retry tick re-runs the check while mail is still
/// pending and the harness might become ready.
export const MAIL_RETRY_INTERVAL_MS = 2000;

/// How long the retry keeps ticking after it was last armed (by an
/// event trigger, or by mail newly becoming pending) before giving up.
/// A harness that hasn't reached a safe stopping point in this long is
/// presumed stuck for reasons this timer can't fix (exited without an
/// `exited` phase update reaching it yet, wedged, or simply a very
/// long-running turn) — the next real event trigger re-arms it.
export const MAIL_RETRY_WINDOW_MS = 60_000;

export interface NextMailRetryInput {
	/// Whether this harness still has mail outstanding as of the last
	/// check (`mailPending`'s own verdict).
	pending: boolean;
	/// The harness's current activity phase, or `null` if it has none
	/// (never spawned, or its record was dropped).
	phase: ActivityPhase | null;
	/// When this retry sequence was (re-)armed, in the same clock as
	/// `nowMs`.
	armedAtMs: number;
	/// The current time.
	nowMs: number;
}

export type NextMailRetryDecision = { kind: "stop" } | { kind: "wait"; delayMs: number };

/// Whether to schedule another retry tick, and after how long. Stops
/// when there's nothing left to retry for (`!pending`), when the
/// harness is gone for good (`phase === "exited"` or no activity
/// record at all), or once the retry window has elapsed since it was
/// armed — `>=`, so a tick landing exactly on the boundary stops rather
/// than scheduling one more. Otherwise waits `MAIL_RETRY_INTERVAL_MS`:
/// deliberately not narrowed to "only while spawning" or any other
/// specific phase, since the whole point is re-trying the unchanged
/// gate wherever it might have started passing — `permission`,
/// mid-turn, a held draft — without this module having to know why the
/// gate refused.
export function nextMailRetry(input: NextMailRetryInput): NextMailRetryDecision {
	const { pending, phase, armedAtMs, nowMs } = input;
	if (!pending || phase === null || phase === "exited") return { kind: "stop" };
	if (nowMs - armedAtMs >= MAIL_RETRY_WINDOW_MS) return { kind: "stop" };
	return { kind: "wait", delayMs: MAIL_RETRY_INTERVAL_MS };
}

/// Whether a harness still has mail worth nudging about — `unread` has
/// grown past `lastNudged`, the same growth test `decideMailNudge`
/// itself applies. Shared here so `useMailDelivery.ts`'s `check` and
/// the retry scheduler agree on one definition of "pending" without
/// duplicating it.
export function mailPending(unread: number, lastNudged: number): boolean {
	return unread > 0 && unread > lastNudged;
}
