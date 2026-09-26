import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { TRANSITION_SOURCE, harnessActivity } from "./harnessActivity.ts";
import { harnessInput, sendPrompt } from "./harnessInput.ts";
import type { HarnessInputTarget } from "./harnessInput.ts";
import { SUBMIT_GAP_MS, SUBMIT_RETRY_AFTER_MS } from "./submitRetry.ts";

// #380 — end-to-end `sendPrompt`: the paste/gate stay synchronous, but
// the submit "\r" now lands `SUBMIT_GAP_MS` later, and a retry-capable
// kind (Claude) gets one more "\r" after `SUBMIT_RETRY_AFTER_MS` if
// nothing proved the first one landed. File-scope fake timers, same
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

	it("retries once for a claude harness still waiting after SUBMIT_RETRY_AFTER_MS", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "claude", "hello");
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		vi.advanceTimersByTime(SUBMIT_RETRY_AFTER_MS);
		expect(target.submit).toHaveBeenCalledTimes(2);
	});

	it("does not retry once the harness has left waiting since the submit", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "claude", "hello");
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		// Proof the first Enter landed — the harness moved on.
		harnessActivity.setRunningFromAdapter(id, "test");

		vi.advanceTimersByTime(SUBMIT_RETRY_AFTER_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);
	});

	it("does not retry once the user has typed since the submit", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "claude", "hello");
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		harnessInput.noteUserInput(id);

		vi.advanceTimersByTime(SUBMIT_RETRY_AFTER_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);
	});

	it("never retries for a kind that doesn't opt in (opencode)", () => {
		const { id, target } = sendableHarness();

		sendPrompt(id, "opencode", "hello");
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		vi.advanceTimersByTime(SUBMIT_RETRY_AFTER_MS);
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

		// The retry window closing afterwards still submits nothing —
		// there was never a first submit to retry.
		vi.advanceTimersByTime(SUBMIT_RETRY_AFTER_MS);
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

		vi.advanceTimersByTime(SUBMIT_RETRY_AFTER_MS);
		expect(target.submit).not.toHaveBeenCalled();
	});
});
