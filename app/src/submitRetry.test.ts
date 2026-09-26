import { describe, expect, it } from "vitest";
import { decideSubmitRetry } from "./submitRetry.ts";
import type { DecideSubmitRetryInput } from "./submitRetry.ts";

// Pure `decideSubmitRetry` tests build the input by hand — no store,
// no DOM, no timers — exactly what the #380 retry policy is supposed
// to allow.

const input = (over: Partial<DecideSubmitRetryInput> = {}): DecideSubmitRetryInput => ({
	capable: true,
	leftWaitingSinceSend: false,
	phase: "waiting",
	userInputSinceSend: false,
	sameTarget: true,
	...over,
});

describe("decideSubmitRetry", () => {
	it("retries when nothing proves the first Enter landed", () => {
		expect(decideSubmitRetry(input())).toEqual({ retry: true });
	});

	it("refuses once the harness left waiting — the first Enter landed", () => {
		const r = decideSubmitRetry(input({ leftWaitingSinceSend: true }));
		expect(r.retry).toBe(false);
	});

	it("refuses when the user typed since the submit", () => {
		const r = decideSubmitRetry(input({ userInputSinceSend: true }));
		expect(r.retry).toBe(false);
	});

	it("refuses a permission dialog rather than treating it like any other non-waiting phase", () => {
		const r = decideSubmitRetry(input({ phase: "permission" }));
		expect(r).toEqual({ retry: false, reason: expect.stringContaining("permission") });
	});

	it.each(["running", "idle", "spawning", "exited"] as const)("refuses phase %s", (phase) => {
		const r = decideSubmitRetry(input({ phase }));
		expect(r.retry).toBe(false);
	});

	it("refuses a kind that doesn't opt in (e.g. opencode)", () => {
		const r = decideSubmitRetry(input({ capable: false }));
		expect(r.retry).toBe(false);
	});

	it("refuses when the target registration changed or is gone", () => {
		const r = decideSubmitRetry(input({ sameTarget: false }));
		expect(r.retry).toBe(false);
	});

	it("refuses when there's no activity record at all (phase null)", () => {
		const r = decideSubmitRetry(input({ phase: null }));
		expect(r.retry).toBe(false);
		if (!r.retry) expect(r.reason).toContain("no activity record");
	});
});
