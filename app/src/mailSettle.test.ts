import { describe, expect, it } from "vitest";
import {
	NUDGE_SETTLE_MS,
	evaluateSettlement,
	settleNudge,
	shouldRecoverSilence,
} from "./mailSettle.ts";
import type {
	EvaluateSettlementInput,
	SettleNudgeInput,
	SettlementActivity,
} from "./mailSettle.ts";

// Pure `settleNudge` tests build the input by hand — no store, no DOM,
// no timers.

const input = (over: Partial<SettleNudgeInput> = {}): SettleNudgeInput => ({
	elapsedMs: 0,
	watched: true,
	turnStartedSinceSend: false,
	phase: "waiting",
	deferredAtSend: null,
	deferredAtNow: null,
	...over,
});

describe("settleNudge", () => {
	it("waits when nothing has happened yet and the deadline hasn't passed", () => {
		expect(settleNudge(input({ elapsedMs: 0 }))).toBe("wait");
	});

	it("waits just under the deadline", () => {
		expect(settleNudge(input({ elapsedMs: NUDGE_SETTLE_MS - 1 }))).toBe("wait");
	});

	it("rolls back exactly at the deadline — >=, not >", () => {
		expect(settleNudge(input({ elapsedMs: NUDGE_SETTLE_MS }))).toBe("rollback");
	});

	it("rolls back once the deadline has passed with no proof", () => {
		expect(settleNudge(input({ elapsedMs: NUDGE_SETTLE_MS + 1_000 }))).toBe("rollback");
	});

	it("confirms once a turn has started since send", () => {
		expect(settleNudge(input({ elapsedMs: 500, turnStartedSinceSend: true }))).toBe("confirmed");
	});

	it("confirms as soon as a turn starts, even at elapsedMs 0", () => {
		expect(settleNudge(input({ elapsedMs: 0, turnStartedSinceSend: true }))).toBe("confirmed");
	});

	it("confirms once the delegation-deferred arm has changed since send", () => {
		expect(settleNudge(input({ elapsedMs: 500, deferredAtSend: 100, deferredAtNow: 200 }))).toBe(
			"confirmed",
		);
	});

	it("confirms when the deferral was disarmed (now null) since send", () => {
		expect(settleNudge(input({ elapsedMs: 500, deferredAtSend: 100, deferredAtNow: null }))).toBe(
			"confirmed",
		);
	});

	it("does not confirm on the deferred arm when it hasn't changed", () => {
		expect(settleNudge(input({ elapsedMs: 500, deferredAtSend: 100, deferredAtNow: 100 }))).toBe(
			"wait",
		);
	});

	it("does not confirm on the deferred arm when both are null (no deferral at all)", () => {
		expect(settleNudge(input({ elapsedMs: 500, deferredAtSend: null, deferredAtNow: null }))).toBe(
			"wait",
		);
	});

	describe("phase gone or respawned beats every other rule", () => {
		it.each([null, "exited", "spawning"] as const)("rolls back for phase %s", (phase) => {
			expect(
				settleNudge(
					input({
						phase,
						turnStartedSinceSend: true,
						deferredAtSend: 100,
						deferredAtNow: 200,
						elapsedMs: 0,
					}),
				),
			).toBe("rollback");
		});
	});

	describe("!watched beats turnStartedSinceSend", () => {
		it("rolls back when unwatched even though a turn appears to have started", () => {
			expect(settleNudge(input({ watched: false, turnStartedSinceSend: true, elapsedMs: 0 }))).toBe(
				"rollback",
			);
		});

		it("rolls back when unwatched even though the deferred arm changed", () => {
			expect(
				settleNudge(
					input({
						watched: false,
						deferredAtSend: 100,
						deferredAtNow: 200,
						elapsedMs: 0,
					}),
				),
			).toBe("rollback");
		});

		it("still rolls back when unwatched and under the deadline — no wait once unwatched", () => {
			expect(settleNudge(input({ watched: false, elapsedMs: 0 }))).toBe("rollback");
		});
	});

	describe("turnStartedSinceSend beats the deferred-arm rule and the deadline", () => {
		it("confirms via turn start even when the deferred arm didn't change", () => {
			expect(
				settleNudge(
					input({
						turnStartedSinceSend: true,
						deferredAtSend: 100,
						deferredAtNow: 100,
						elapsedMs: NUDGE_SETTLE_MS + 1,
					}),
				),
			).toBe("confirmed");
		});
	});

	it.each(["running", "permission", "idle"] as const)(
		"still waits for phase %s with no other evidence and time left",
		(phase) => {
			expect(settleNudge(input({ phase, elapsedMs: 0 }))).toBe("wait");
		},
	);

	// #388's own guarantee must still hold with #404 layered on top: an
	// unwatched turn-start never confirms, regardless of what a #404
	// recovery attempt might otherwise do with the rollback that follows.
	it("still rolls back, never confirms, when unwatched even though a turn appears to have started", () => {
		expect(settleNudge(input({ watched: false, turnStartedSinceSend: true, elapsedMs: 0 }))).toBe(
			"rollback",
		);
	});
});

// #404: the pure policy over whether a rollback should also attempt
// recovering a lost-silence degrade — see `shouldRecoverSilence`'s own
// doc for what it deliberately does NOT decide (the store-side
// conditions live in `harnessActivity.recoverUnheardSilence`).
describe("shouldRecoverSilence", () => {
	it("recovers on rollback with no user input since send", () => {
		expect(shouldRecoverSilence({ outcome: "rollback", userInputSinceSend: false })).toBe(true);
	});

	it("refuses on rollback when the user drove the harness since send", () => {
		expect(shouldRecoverSilence({ outcome: "rollback", userInputSinceSend: true })).toBe(false);
	});

	it("refuses for a confirmed nudge — nothing to recover from", () => {
		expect(shouldRecoverSilence({ outcome: "confirmed", userInputSinceSend: false })).toBe(false);
	});

	it("refuses while still waiting — the outcome isn't a rollback yet", () => {
		expect(shouldRecoverSilence({ outcome: "wait", userInputSinceSend: false })).toBe(false);
	});
});

// #404: `evaluateSettlement` is the pure decision half of
// `useMailDelivery.ts`'s settlement block — built on `settleNudge` and
// `shouldRecoverSilence` above, table-tested the same way.
describe("evaluateSettlement", () => {
	const watchedActivity: SettlementActivity = {
		authoritative: true,
		adapterSilent: false,
		phase: "waiting",
		delegationDeferredAt: null,
	};

	const evalInput = (over: Partial<EvaluateSettlementInput> = {}): EvaluateSettlementInput => ({
		nowMs: NUDGE_SETTLE_MS + 1,
		settlement: {
			preNudge: 3,
			sentAtMs: 0,
			deferredAtSend: null,
			turnStarted: false,
			inputCountAtSend: 1,
		},
		activity: watchedActivity,
		inputCountNow: 1,
		...over,
	});

	it("rollback with unchanged input count: attemptRecovery true, restoreLastNudged = preNudge", () => {
		const result = evaluateSettlement(evalInput());
		expect(result.outcome).toBe("rollback");
		expect(result.restoreLastNudged).toBe(3);
		expect(result.attemptRecovery).toBe(true);
		expect(result.settlementPending).toBe(false);
	});

	it("rollback with user input since send: attemptRecovery false", () => {
		const result = evaluateSettlement(evalInput({ inputCountNow: 2 }));
		expect(result.outcome).toBe("rollback");
		expect(result.restoreLastNudged).toBe(3);
		expect(result.attemptRecovery).toBe(false);
	});

	it("confirmed: no restore, no recovery attempt", () => {
		const result = evaluateSettlement(
			evalInput({
				nowMs: 500,
				settlement: {
					preNudge: 3,
					sentAtMs: 0,
					deferredAtSend: null,
					turnStarted: true,
					inputCountAtSend: 1,
				},
			}),
		);
		expect(result.outcome).toBe("confirmed");
		expect(result.restoreLastNudged).toBeNull();
		expect(result.attemptRecovery).toBe(false);
		expect(result.settlementPending).toBe(false);
	});

	it("wait: settlementPending true, no restore, no recovery attempt", () => {
		const result = evaluateSettlement(evalInput({ nowMs: 500 }));
		expect(result.outcome).toBe("wait");
		expect(result.restoreLastNudged).toBeNull();
		expect(result.attemptRecovery).toBe(false);
		expect(result.settlementPending).toBe(true);
	});

	it("null activity: rollback (unwatched, same as settleNudge's own !watched rule)", () => {
		const result = evaluateSettlement(evalInput({ nowMs: 500, activity: null }));
		expect(result.outcome).toBe("rollback");
		expect(result.restoreLastNudged).toBe(3);
	});

	// The #404 sequence this issue exists for: a silent-degraded adapter
	// (`adapterSilent: true`, so `watched` is false) that ALSO shows
	// `turnStarted: true` — the very case #388's `!watched` rule outranks
	// `turnStartedSinceSend` for, since a degraded adapter's own L2a
	// fallback can manufacture a "turn started" reading from bare PTY
	// output that is not proof the nudge was received. Must still roll
	// back, never confirm.
	it("#404: unwatched (adapterSilent) with turnStarted true still rolls back, never confirms", () => {
		const result = evaluateSettlement(
			evalInput({
				nowMs: 500,
				settlement: {
					preNudge: 3,
					sentAtMs: 0,
					deferredAtSend: null,
					turnStarted: true,
					inputCountAtSend: 1,
				},
				activity: { ...watchedActivity, adapterSilent: true },
			}),
		);
		expect(result.outcome).toBe("rollback");
		expect(result.restoreLastNudged).toBe(3);
	});
});
