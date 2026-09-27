import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { harnessActivity } from "./harnessActivity.ts";
import { ADAPTER_SILENT_AFTER_MS } from "./harnessActivityConstants.ts";
import { harnessInput, sendPrompt } from "./harnessInput.ts";
import type { HarnessInputTarget } from "./harnessInput.ts";
import { LAUNCH_QUIET_MS } from "./launchReady.ts";
import { evaluateSettlement } from "./mailSettle.ts";
import { SUBMIT_GAP_MS } from "./submitRetry.ts";

// #404, end-to-end through the real seam: a mail nudge into a fresh
// harness whose only proof of life is the launch signal arms the #259
// watchdog on submit; when that submit never lands (what
// `useMailDelivery.ts`'s rollback calls `recoverUnheardSilence` for),
// the harness must come back to a sendable state rather than staying
// permanently refused. Fake timers at file scope, same reason
// `harnessInput.watchdog.test.ts` gives — the tick's `setInterval` is
// created by the first `harnessActivity.spawned()` call.
beforeAll(() => {
	vi.useFakeTimers();
});
afterAll(() => {
	vi.useRealTimers();
});

const nextId = (() => {
	let n = 0;
	return () => `rec_${++n}`;
})();

describe("recovering an unheard silence through the real seam (#404)", () => {
	it("refuses after a silent degrade, recovers, sends again, then degrades for good on a second silence", () => {
		const id = nextId();
		const target: HarnessInputTarget = {
			paste: vi.fn(),
			bracketedPaste: () => false,
			submit: vi.fn(),
		};
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		harnessActivity.noteLaunchSignal(id);
		harnessActivity.setInjected(id, true);
		harnessInput.register(id, target);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");

		// #404: the launch-settle quiet window before a first paste is
		// trusted at all.
		vi.advanceTimersByTime(LAUNCH_QUIET_MS);

		// `opencode` deliberately, not `claude`: it has no
		// `capabilities.submitRetry`, so each `sendPrompt` below writes
		// exactly one submit — the #380 retry schedule is orthogonal to
		// what this test is about and would only make the submit counts
		// below noisy.
		//
		// First send: gate passes, paste lands, and (after the #380 gap)
		// the submit lands too — this is the nudge that's about to be
		// "lost" from the adapter's point of view.
		let result = sendPrompt(id, "opencode", "you have mail");
		expect(result).toEqual({ ok: true });
		expect(target.paste).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);
		expect(harnessActivity.get(id)?.promptSubmittedAt).not.toBeNull();

		// The adapter never answers — #259 degrades the harness.
		vi.advanceTimersByTime(ADAPTER_SILENT_AFTER_MS + 1_000);
		const degraded = harnessActivity.get(id);
		expect(degraded?.adapterSilent).toBe(true);
		expect(degraded?.degradedBy).toBe("adapter-silent");
		expect(degraded?.authoritative).toBe(false);

		// A retry is refused outright — nothing is confirmed to be
		// watching this harness any more.
		result = sendPrompt(id, "opencode", "you have mail");
		expect(result).toEqual({
			ok: false,
			reason: "no confirmed adapter is watching this harness",
		});
		expect(target.paste).toHaveBeenCalledTimes(1);

		// This is what `useMailDelivery.ts`'s rollback branch calls once
		// it has confirmed no human drove the harness in the meantime.
		expect(harnessActivity.recoverUnheardSilence(id)).toBe(true);
		const recovered = harnessActivity.get(id);
		expect(recovered?.authoritative).toBe(true);
		expect(recovered?.adapterSilent).toBe(false);
		expect(recovered?.degradedBy).toBeNull();
		expect(recovered?.silenceRecovered).toBe(true);

		// The launch-settle gate re-arms against the degrade's own
		// `lastOutputAt` bump — give it its quiet window again before the
		// retry, same as any other post-recovery send would need to.
		vi.advanceTimersByTime(LAUNCH_QUIET_MS);

		// The retry now succeeds and pastes again.
		result = sendPrompt(id, "opencode", "you have mail");
		expect(result).toEqual({ ok: true });
		expect(target.paste).toHaveBeenCalledTimes(2);
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(2);

		// A SECOND silence on the same spawn is real evidence, not
		// another lost race — it degrades, and this time recovery refuses.
		vi.advanceTimersByTime(ADAPTER_SILENT_AFTER_MS + 1_000);
		const degradedAgain = harnessActivity.get(id);
		expect(degradedAgain?.adapterSilent).toBe(true);
		expect(degradedAgain?.degradedBy).toBe("adapter-silent");

		expect(harnessActivity.recoverUnheardSilence(id)).toBe(false);
		result = sendPrompt(id, "opencode", "you have mail");
		expect(result).toEqual({
			ok: false,
			reason: "no confirmed adapter is watching this harness",
		});
		expect(target.paste).toHaveBeenCalledTimes(2);
	});

	// #404: the same chain, but driven through `evaluateSettlement` itself
	// — `useMailDelivery.ts`'s `check()` never calls
	// `recoverUnheardSilence` directly; it only does so on
	// `evaluateSettlement`'s say-so. This proves that pure decision wires
	// up to the real store and seam the same way the hand-driven test
	// above does.
	it("evaluateSettlement's rollback+attemptRecovery reopens the gate for a real sendPrompt retry", () => {
		const id = nextId();
		const target: HarnessInputTarget = {
			paste: vi.fn(),
			bracketedPaste: () => false,
			submit: vi.fn(),
		};
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		harnessActivity.noteLaunchSignal(id);
		harnessActivity.setInjected(id, true);
		harnessInput.register(id, target);

		vi.advanceTimersByTime(LAUNCH_QUIET_MS);

		const inputCountAtSend = harnessInput.userInputCount(id);
		const sentAtMs = Date.now();
		const result = sendPrompt(id, "opencode", "you have mail");
		expect(result).toEqual({ ok: true });
		const deferredAtSend = harnessActivity.get(id)?.delegationDeferredAt ?? null;
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		// The adapter never answers — #259 degrades the harness, the same
		// as `useMailDelivery.ts` would see on its next `check`.
		vi.advanceTimersByTime(ADAPTER_SILENT_AFTER_MS + 1_000);
		const activity = harnessActivity.get(id);
		expect(activity?.adapterSilent).toBe(true);

		const decision = evaluateSettlement({
			nowMs: Date.now(),
			settlement: {
				preNudge: 0,
				sentAtMs,
				deferredAtSend,
				turnStarted: false,
				inputCountAtSend,
			},
			activity:
				activity === null
					? null
					: {
							authoritative: activity.authoritative,
							adapterSilent: activity.adapterSilent,
							phase: activity.phase,
							delegationDeferredAt: activity.delegationDeferredAt,
						},
			inputCountNow: harnessInput.userInputCount(id),
		});
		expect(decision.outcome).toBe("rollback");
		expect(decision.attemptRecovery).toBe(true);
		expect(decision.restoreLastNudged).toBe(0);

		expect(harnessActivity.recoverUnheardSilence(id)).toBe(true);

		// Same quiet window a fresh recovery needs before the gate trusts
		// a paste again.
		vi.advanceTimersByTime(LAUNCH_QUIET_MS);

		const retry = sendPrompt(id, "opencode", "you have mail");
		expect(retry).toEqual({ ok: true });
		expect(target.paste).toHaveBeenCalledTimes(2);
	});
});
