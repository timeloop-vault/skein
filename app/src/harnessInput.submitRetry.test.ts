import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { TRANSITION_SOURCE, harnessActivity } from "./harnessActivity.ts";
import { harnessInput, sendPrompt } from "./harnessInput.ts";
import type { HarnessInputTarget } from "./harnessInput.ts";
import { SUBMIT_GAP_MS, SUBMIT_RETRY_SCHEDULE_MS } from "./submitRetry.ts";

// #380 — end-to-end `sendPrompt`: the paste/gate stay synchronous, but
// the submit "\r" now lands `SUBMIT_GAP_MS` later, and a retry-capable
// kind (Claude) gets up to three more "\r"s at `SUBMIT_RETRY_SCHEDULE_MS`
// if nothing proved an earlier one landed. File-scope fake timers, same
// reason `harnessInput.watchdog.test.ts` uses them: the activity
// store's own idle tick is a module-global `setInterval` created by
// the first `harnessActivity.spawned()` call, so it has to come up
// under whichever timer implementation is live at that moment.
beforeAll(() => {
	vi.useFakeTimers();
});
afterAll(() => {
	vi.useRealTimers();
});

const nextId = (() => {
	let n = 0;
	return () => `sr_${++n}`;
})();

/// The window past the last scheduled retry — used to prove nothing
/// fires beyond the schedule.
const PAST_SCHEDULE_MS =
	(SUBMIT_RETRY_SCHEDULE_MS[SUBMIT_RETRY_SCHEDULE_MS.length - 1] ?? 0) + 5_000;

/// A registered, sendable harness sitting in `waiting`, authoritative,
/// heard-from and injected — the one state `canSendPrompt` allows.
const sendableHarness = (): { id: string; target: HarnessInputTarget } => {
	const id = nextId();
	const target: HarnessInputTarget = {
		paste: vi.fn(),
		bracketedPaste: () => false,
		submit: vi.fn(),
	};
	harnessActivity.spawned(id);
	harnessActivity.attachAuthoritativeSource(id);
	harnessActivity.adapterDelivered(id);
	harnessActivity.setWaitingFromAdapter(id, "test");
	harnessActivity.setInjected(id, true);
	harnessInput.register(id, target);
	return { id, target };
};

describe("sendPrompt's #380 gap + retry", () => {
	it("submits only after the gap — never in the same tick as the paste", () => {
		const { id, target } = sendableHarness();

		const result = sendPrompt(id, "claude", "hello");

		expect(result).toEqual({ ok: true });
		expect(target.paste).toHaveBeenCalledTimes(1);
		expect(target.submit).not.toHaveBeenCalled();

		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);
	});

	it("retries at every scheduled delay while still waiting — three total, never a fourth", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "claude", "hello");
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		let elapsedSinceSubmit = 0;
		SUBMIT_RETRY_SCHEDULE_MS.forEach((delay, i) => {
			vi.advanceTimersByTime(delay - elapsedSinceSubmit);
			elapsedSinceSubmit = delay;
			expect(target.submit).toHaveBeenCalledTimes(2 + i);
		});

		// Nothing left scheduled past the last entry.
		vi.advanceTimersByTime(PAST_SCHEDULE_MS - elapsedSinceSubmit);
		expect(target.submit).toHaveBeenCalledTimes(1 + SUBMIT_RETRY_SCHEDULE_MS.length);
	});

	it("stops after the first retry once the harness has left waiting", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "claude", "hello");
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		vi.advanceTimersByTime(SUBMIT_RETRY_SCHEDULE_MS[0]);
		expect(target.submit).toHaveBeenCalledTimes(2);

		// Proof the Enter landed — the harness moved on. Every later
		// scheduled attempt must refuse from here, not just the next one.
		harnessActivity.setRunningFromAdapter(id, "test");

		vi.advanceTimersByTime(PAST_SCHEDULE_MS - (SUBMIT_RETRY_SCHEDULE_MS[0] ?? 0));
		expect(target.submit).toHaveBeenCalledTimes(2);
	});

	it("stops once the user has typed, even mid-schedule", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "claude", "hello");
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		vi.advanceTimersByTime(SUBMIT_RETRY_SCHEDULE_MS[0]);
		expect(target.submit).toHaveBeenCalledTimes(2);

		harnessInput.noteUserInput(id);

		vi.advanceTimersByTime(PAST_SCHEDULE_MS - (SUBMIT_RETRY_SCHEDULE_MS[0] ?? 0));
		expect(target.submit).toHaveBeenCalledTimes(2);
	});

	it("never retries for a kind that doesn't opt in (opencode)", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "opencode", "hello");
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		vi.advanceTimersByTime(PAST_SCHEDULE_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);
	});

	// The store has no direct "degrade this adapter" API to call outside
	// the #259 watchdog's own 10s timer, which this file's window can't
	// safely cross without also risking a real watchdog fire mid-test —
	// so this drives the same underlying state (`authoritative: false`)
	// the watchdog would leave behind, via `detachAuthoritativeSource`.
	// `decideSubmitRetry`'s own table test separately covers
	// `watched: false` as a pure input.
	it("stops retrying once nothing is confirmed to be watching the harness any more", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "claude", "hello");
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		harnessActivity.detachAuthoritativeSource(id);

		vi.advanceTimersByTime(PAST_SCHEDULE_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);
	});

	it("does not submit at all if a permission dialog opens during the gap", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "claude", "hello");
		harnessActivity.setPermissionFromAdapter(
			id,
			TRANSITION_SOURCE.L2c1ClaudePermission,
			"Bash",
			null,
			"agent-1",
		);

		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).not.toHaveBeenCalled();

		// The retry schedule closing afterwards still submits nothing —
		// there was never a first submit to retry.
		vi.advanceTimersByTime(PAST_SCHEDULE_MS);
		expect(target.submit).not.toHaveBeenCalled();
	});

	// Review follow-up: the deferred-submit recheck mirrors
	// `canSendPrompt` generally — any phase drift off `waiting` skips
	// the submit, not only a permission dialog opening.
	it("does not submit at all if the phase moves to running during the gap", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "claude", "hello");
		harnessActivity.setRunningFromAdapter(id, "test");

		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).not.toHaveBeenCalled();

		vi.advanceTimersByTime(PAST_SCHEDULE_MS);
		expect(target.submit).not.toHaveBeenCalled();
	});
});
