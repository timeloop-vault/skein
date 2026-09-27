import { describe, expect, it } from "vitest";
import { NUDGE_SETTLE_MS, settleNudge } from "./mailSettle.ts";
import type { SettleNudgeInput } from "./mailSettle.ts";

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
});
