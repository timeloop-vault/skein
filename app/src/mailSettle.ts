// #388: a mail nudge sent through the #238 seam must count as delivered
// only once its submit is actually seen to land — `sendPrompt`'s own
// `result.ok` proves only that the gate passed and the body was
// *pasted*; the submit "\r" follows `SUBMIT_GAP_MS` later and can still
// be silently skipped (harnessInput.ts's #380 gap-then-retry comment).
// `useMailDelivery.ts`'s `check()` used to record `lastNudged` straight
// off `result.ok`, so a swallowed submit permanently under-counted:
// the text never reached the harness, but `mailRetry.ts`'s
// `mailPending` read the mail as already handled and #386's bounded
// retry never got another look at it.
//
// This module is pure and DOM-free — no store, no timers — the same
// split `submitRetry.ts` and `mailRetry.ts` already draw. It answers
// one question: has enough evidence arrived, since a nudge's paste, to
// treat the send as either confirmed or as never having landed
// (`rollback`, meaning `useMailDelivery.ts` puts `lastNudged` back to
// its pre-nudge value so `decideMailNudge` reconsiders it)? The third
// answer, `wait`, means neither is known yet.
//
// The evidence is the same shape `submitRetry.ts` already uses to
// decide whether ITS OWN retries are still safe: a transition leaving
// `waiting`, or a change to the #277 delegation-deferred arm the send
// was made under. Reusing it here means a nudge settles on exactly the
// same proof a submit retry would have relied on, not a second,
// possibly disagreeing, notion of "landed".

import type { ActivityPhase } from "./harnessActivityTypes.ts";

/// How long `useMailDelivery.ts` waits, from the moment a nudge is
/// PASTED, before giving up on proof it landed and rolling the nudge
/// back. Two things it must outlast:
///
///  - the #259 silent-adapter watchdog's LATEST possible firing for
///    this send: `SUBMIT_GAP_MS` (150 ms) before the submit is even
///    attempted, plus `ADAPTER_SILENT_AFTER_MS` (10 s) of watched
///    silence, plus one more watchdog tick (1 s) before it's actually
///    observed — call it 150 ms + 10 s + 1 s ≈ 11.15 s. Settling any
///    earlier risks reading `watched` as still true when the watchdog
///    was about to degrade it right after;
///  - `submitRetry.ts`'s own last scheduled attempt, at 8 s from the
///    submit (which itself starts `SUBMIT_GAP_MS` after the paste) —
///    so settlement must run only once a Claude harness's own retry
///    schedule has already had its say.
///
/// 13 s clears both with room to spare, so by the time `settleNudge`
/// runs, a degraded adapter is already visible as degraded (`watched`
/// is already false, not still transitioning) rather than racing the
/// watchdog's own tick.
export const NUDGE_SETTLE_MS = 13_000;

/// What `settleNudge` needs to decide, gathered by the caller so this
/// stays pure — `useMailDelivery.ts` owns the actual timer and reads
/// `harnessActivity` state to build these.
export interface SettleNudgeInput {
	/// Milliseconds since the nudge was pasted.
	elapsedMs: number;
	/// Is anything still confirmed to be watching this harness —
	/// `activity.authoritative && !activity.adapterSilent`, or `false`
	/// if there's no activity record at all. The same #259 watchdog
	/// proof `submitRetry.ts`'s own `watched` check relies on.
	watched: boolean;
	/// Did a transition from `waiting` to `running` or `permission`
	/// land for this harness at any point after the nudge was sent?
	/// That's the harness starting a new turn (or opening a dialog),
	/// which only happens once it received input — the same proof
	/// `submitRetry.ts` uses to end its own retry sequence, read the
	/// other way round: there, leaving `waiting` stops a retry that
	/// would otherwise duplicate a landed Enter; here, it's exactly
	/// what confirms the nudge's Enter landed at all.
	turnStartedSinceSend: boolean;
	/// The harness's phase right now, or `null` if it has no activity
	/// record at all (never spawned, or the record was dropped).
	phase: ActivityPhase | null;
	/// The #277 delegation deferral (if any) this nudge was pasted
	/// under — the caller's `delegationDeferredAt` snapshot taken at
	/// paste time, or `null` for a nudge pasted at a plain `waiting`.
	deferredAtSend: number | null;
	/// The harness's current `delegationDeferredAt`, sampled fresh at
	/// decision time.
	deferredAtNow: number | null;
}

export type SettleNudgeDecision = "confirmed" | "rollback" | "wait";

/// Pure policy, rules applied in order — each one below is a rollback
/// or confirmation that outranks every rule after it:
///
///  1. The harness is gone or has respawned (`phase` is `null`,
///     `"exited"` or `"spawning"`) → `"rollback"`. The paste died with
///     the old PTY (or there never was one to paste into), so nothing
///     can still land.
///  2. Nothing is confirmed to be watching (`!watched`) → `"rollback"`.
///     Once the #259 watchdog has degraded the adapter (or there's no
///     activity record to have `authoritative` in the first place),
///     the watchdog firing is itself proof no transcript row appeared
///     for this send — and after a degrade, the L2a idle heuristic can
///     move `waiting → running` on bare PTY output, which is NOT proof
///     the nudge was received. That's why this rule outranks rule 3:
///     an unwatched "turn started" is exactly the case a degraded
///     adapter can manufacture.
///  3. `turnStartedSinceSend` → `"confirmed"`. A watched transition out
///     of `waiting` is proof the Enter reached the harness.
///  4. The #277 delegation-deferred arm changed since send
///     (`deferredAtSend !== null && deferredAtNow !== deferredAtSend`)
///     → `"confirmed"`. The same conservative "arm changed" rule
///     `submitRetry.ts` uses to end its own retries — a disarm, a
///     flush to `waiting`, or a fresh re-arm all mean real work
///     happened since the paste. This is deliberately read as
///     confirmation, not as its own kind of ambiguity: a nudge pasted
///     into a session that's actively delegating and stays that way is
///     not the field case this settle window exists for — a swallowed
///     paste into an otherwise-dead composer is.
///  5. Still under the deadline (`elapsedMs < NUDGE_SETTLE_MS`) →
///     `"wait"` — no proof either way yet.
///  6. Otherwise → `"rollback"`. The deadline passed with no proof the
///     submit landed, so the nudge goes back to pending.
export function settleNudge(input: SettleNudgeInput): SettleNudgeDecision {
	const { elapsedMs, watched, turnStartedSinceSend, phase, deferredAtSend, deferredAtNow } = input;
	if (phase === null || phase === "exited" || phase === "spawning") return "rollback";
	if (!watched) return "rollback";
	if (turnStartedSinceSend) return "confirmed";
	if (deferredAtSend !== null && deferredAtNow !== deferredAtSend) return "confirmed";
	if (elapsedMs < NUDGE_SETTLE_MS) return "wait";
	return "rollback";
}
