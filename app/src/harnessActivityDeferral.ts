// harnessActivity.ts split (#458): adapter work/end-of-turn signals, the #277/#446 deferral and delegation counters. Methods are spread into the
// `harnessActivity` store object there; shared state lives in harnessActivityCore.ts.

import { outstandingWork } from "./deferral.ts";
import {
	delegationDeferredListeners,
	disarmDelegation,
	setPhase,
	store,
} from "./harnessActivityCore.ts";
import type { TransitionSource } from "./harnessActivityTypes.ts";

export const deferralMethods = {
	/// L2c adapter says the harness is doing work. Bypasses the
	/// recordOutput throttle so the transition fires on the
	/// adapter event boundary, not on the next PTY chunk. The
	/// `source` identifies which adapter event drove the transition
	/// (e.g. `l2c1-claude-assistant`, `l2c2-opencode-busy`) and
	/// flows through to the persisted event log for L7 attribution.
	///
	/// #86: a harness sitting in `permission` is hard-blocked on a
	/// dialog the user hasn't answered yet. Most adapter events that
	/// call this just mean "still working" (a fresh assistant turn, the
	/// next tool queued) and must not paper over that — so by default
	/// this is a no-op while in `permission`. Only a signal that
	/// genuinely means the gate is gone (a tool result landed, the
	/// user submitted a fresh prompt) should pass `clearsPermission:
	/// true`; the translator at each call site decides which it is.
	setRunningFromAdapter(
		id: string,
		source: TransitionSource,
		opts?: { clearsPermission?: boolean },
	): void {
		// #277 Rule 2: any main-transcript work signal disarms a
		// deferred end-of-turn, even one this call is about to no-op
		// on below (still `permission` without `clearsPermission`) —
		// the disarm must not depend on `setPhase` actually running.
		disarmDelegation(id);
		const cur = store.get(id);
		// #423: record what the adapter said before any guard can drop it.
		if (cur) cur.lastTurnSignal = { kind: "work", at: Date.now() };
		if (!cur || cur.phase === "exited") return;
		if (cur.phase === "permission" && !opts?.clearsPermission) return;
		setPhase(id, "running", source);
	},

	/// L2c adapter says the harness is awaiting user input. Bypasses
	/// the chunk throttle for the same reason. Always allowed to leave
	/// `permission` — unlike `setRunningFromAdapter`, every signal that
	/// calls this (Claude's end-of-turn, opencode's session-idle) means
	/// the harness is unambiguously done with whatever it was doing,
	/// permission gate included. #86.
	setWaitingFromAdapter(id: string, source: TransitionSource): void {
		// #277 Rule 2: unconditional disarm, same reasoning as
		// `setRunningFromAdapter` above.
		disarmDelegation(id);
		const cur = store.get(id);
		// #423: see `setRunningFromAdapter`.
		if (cur) cur.lastTurnSignal = { kind: "end", at: Date.now() };
		if (!cur || cur.phase === "exited") return;
		setPhase(id, "waiting", source);
	},

	/// #277 (epic #298): the Claude translator's `awaiting_prompt` arm
	/// calls this instead of `setWaitingFromAdapter` directly. The main
	/// transcript ended its turn, but that is a lie about the harness
	/// being done if it just delegated work still running in the
	/// background — measured true in 94% of 127 real sessions.
	///
	/// No outstanding work (no working subagents or background tasks): identical
	/// to today, straight to `waiting`. One or more: arm the deferral
	/// (idempotent — a second end-of-turn while already deferred
	/// doesn't reset when it was armed) and change NO phase at all.
	/// The harness is already `running`, or `permission` — which
	/// outranks this regardless, so simply not calling `setPhase`
	/// leaves it alone either way. See the tick's Rules 3/4 for how a
	/// deferral eventually resolves without a further adapter event.
	awaitingPromptFromAdapter(id: string, source: TransitionSource): void {
		const cur = store.get(id);
		// #423: an end of turn is recorded even when it only arms the
		// deferral and changes no phase.
		if (cur) cur.lastTurnSignal = { kind: "end", at: Date.now() };
		if (!cur || cur.phase === "exited") return;
		if (outstandingWork(id) === 0) {
			deferralMethods.setWaitingFromAdapter(id, source);
			return;
		}
		if (cur.delegationDeferredAt === null) {
			cur.delegationDeferredAt = Date.now();
			// #381: notify arm-only listeners — see
			// `subscribeDelegationDeferred` below. Must sit inside this
			// guard, not after it, so the idempotent re-arm case (a second
			// end-of-turn while already deferred) never fires it twice.
			for (const cb of delegationDeferredListeners) cb(id);
		}
	},
	/// #277: a subagent started LIVE (never an attach-time replay — the
	/// translator only calls this for `event.initial !== true`). Bumps
	/// `delegatedCount` (what the notification wording counts) and
	/// `delegationActivityAt` (what the Rule 4 ceiling measures
	/// silence against). Silent mutation, no emit — the way
	/// `recordOutput` bumps `lastOutputAt` — subscribers must not
	/// re-render on subagent chatter, only on the harness's own phase.
	noteSubagentStarted(id: string): void {
		const cur = store.get(id);
		if (!cur) return;
		cur.delegatedCount += 1;
		cur.delegationActivityAt = Date.now();
	},

	/// #277: a subagent produced a tool result or reached its own end
	/// of turn. Bumps `delegationActivityAt` only — proof of life for
	/// the Rule 4 ceiling. Doesn't touch `delegatedCount`: that counts
	/// starts, not activity. Silent mutation, no emit, same reasoning
	/// as `noteSubagentStarted`.
	noteSubagentActivity(id: string): void {
		const cur = store.get(id);
		if (!cur) return;
		cur.delegationActivityAt = Date.now();
	},

	/// #277: the user submitted a fresh prompt to this harness — resets
	/// `delegatedCount` to 0, since it counts delegations since the
	/// LAST prompt. Silent mutation, no emit, same reasoning as
	/// `noteSubagentStarted`.
	noteUserPrompt(id: string): void {
		const cur = store.get(id);
		if (!cur) return;
		cur.delegatedCount = 0;
	},
};
