import { describe, expect, it } from "vitest";
import { ADAPTER_SILENT_AFTER_MS } from "./harnessActivityConstants.ts";
import { SUBMIT_RETRY_SCHEDULE_MS, decideSubmitRetry } from "./submitRetry.ts";
import type { DecideSubmitRetryInput } from "./submitRetry.ts";

// Pure `decideSubmitRetry` tests build the input by hand — no store,
// no DOM, no timers — exactly what the #380 retry policy is supposed
// to allow.

const input = (over: Partial<DecideSubmitRetryInput> = {}): DecideSubmitRetryInput => ({
	capable: true,
	watched: true,
	leftStoppingPointSinceSend: false,
	phase: "waiting",
	deferredAtSend: null,
	deferredAtNow: null,
	userInputSinceSend: false,
	sameTarget: true,
	...over,
});

describe("decideSubmitRetry", () => {
	it("retries when nothing proves the first Enter landed", () => {
		expect(decideSubmitRetry(input())).toEqual({ retry: true });
	});

	it("refuses once the harness left waiting — the first Enter landed", () => {
		const r = decideSubmitRetry(input({ leftStoppingPointSinceSend: true }));
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

	it("refuses once nothing is confirmed to be watching (the #259 watchdog degraded the adapter)", () => {
		const r = decideSubmitRetry(input({ watched: false }));
		expect(r).toEqual({ retry: false, reason: expect.stringContaining("watching") });
	});
});

describe("decideSubmitRetry — #277 delegation-deferred arm (#381)", () => {
	it("retries a running harness whose deferral hasn't changed since send", () => {
		const r = decideSubmitRetry(
			input({ phase: "running", deferredAtSend: 100, deferredAtNow: 100 }),
		);
		expect(r).toEqual({ retry: true });
	});

	it("refuses once the deferral has been disarmed since send", () => {
		const r = decideSubmitRetry(
			input({ phase: "running", deferredAtSend: 100, deferredAtNow: null }),
		);
		expect(r.retry).toBe(false);
	});

	it("refuses once the deferral has been flushed to waiting by the tick's settle/ceiling", () => {
		const r = decideSubmitRetry(
			input({ phase: "waiting", deferredAtSend: 100, deferredAtNow: null }),
		);
		expect(r.retry).toBe(false);
	});

	it("refuses once a fresh deferral has re-armed with a different timestamp", () => {
		const r = decideSubmitRetry(
			input({ phase: "running", deferredAtSend: 100, deferredAtNow: 200 }),
		);
		expect(r.retry).toBe(false);
	});

	it("refuses a plain running phase with no deferral armed at all", () => {
		const r = decideSubmitRetry(
			input({ phase: "running", deferredAtSend: null, deferredAtNow: null }),
		);
		expect(r.retry).toBe(false);
	});

	it("refuses a permission dialog even mid-deferral", () => {
		const r = decideSubmitRetry(
			input({ phase: "permission", deferredAtSend: 100, deferredAtNow: 100 }),
		);
		expect(r.retry).toBe(false);
	});
});

describe("SUBMIT_RETRY_SCHEDULE_MS", () => {
	it("keeps every scheduled retry inside the #259 watchdog's window", () => {
		// `ADAPTER_SILENT_AFTER_MS` lives in harnessActivityConstants.ts,
		// a pure constants module with no store side effects — safe to
		// import directly rather than hardcode, so this test breaks if
		// the two constants ever drift apart.
		for (const delay of SUBMIT_RETRY_SCHEDULE_MS) {
			expect(delay).toBeLessThan(ADAPTER_SILENT_AFTER_MS);
		}
	});
});
