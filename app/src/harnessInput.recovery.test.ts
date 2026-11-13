import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

// #404, through the real seam: a mail nudge into a harness whose submit
// the adapter never answered degrades it (#259, now decided by the daemon,
// #732). `useMailDelivery.ts`'s rollback asks `evaluateSettlement` whether to
// recover, tells the daemon, and the harness is sendable again once the
// daemon's update says so.

const h = vi.hoisted(() => ({
	calls: [] as Array<{ cmd: string; args: Record<string, unknown> }>,
}));

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
	},
	invoke: (cmd: string, args: Record<string, unknown>) => {
		h.calls.push({ cmd, args });
		return Promise.resolve();
	},
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { harnessActivity } from "./harnessActivity.ts";
import type { HarnessInputTarget } from "./harnessInput.ts";
import { harnessInput, sendPrompt } from "./harnessInput.ts";
import { LAUNCH_QUIET_MS } from "./launchReady.ts";
import { evaluateSettlement } from "./mailSettle.ts";
import { mirrorSet } from "./phaseTestSupport.ts";
import { SUBMIT_GAP_MS } from "./submitRetry.ts";

beforeAll(() => {
	vi.useFakeTimers();
});
afterAll(() => {
	vi.useRealTimers();
});
beforeEach(() => {
	h.calls.length = 0;
});

const forwarded = () => h.calls.filter((c) => c.cmd === "harness_phase_input").map((c) => c.args);

describe("recovering an unheard silence through the real seam (#404)", () => {
	it("evaluateSettlement's rollback+attemptRecovery reopens the gate for a real sendPrompt retry", () => {
		const id = "rec_1";
		const target: HarnessInputTarget = {
			paste: vi.fn(),
			bracketedPaste: () => false,
			submit: vi.fn(),
		};
		const live = { authoritative: true, launchSignalAt: Date.now(), injected: true };
		mirrorSet(id, "waiting", live);
		harnessInput.register(id, target);
		vi.advanceTimersByTime(LAUNCH_QUIET_MS);

		const inputCountAtSend = harnessInput.userInputCount(id);
		const sentAtMs = Date.now();
		// `opencode` has no `capabilities.submitRetry`: one submit per send.
		expect(sendPrompt(id, "opencode", "you have mail")).toEqual({ ok: true });
		vi.advanceTimersByTime(SUBMIT_GAP_MS);
		expect(target.submit).toHaveBeenCalledTimes(1);

		// The daemon degrades the harness: the adapter never answered.
		mirrorSet(id, "waiting", {
			...live,
			authoritative: false,
			adapterSilent: true,
		});
		const activity = harnessActivity.get(id);
		expect(activity?.adapterSilent).toBe(true);
		expect(sendPrompt(id, "opencode", "you have mail")).toEqual({
			ok: false,
			reason: "no confirmed adapter is watching this harness",
		});

		const decision = evaluateSettlement({
			nowMs: Date.now(),
			settlement: {
				preNudge: 0,
				sentAtMs,
				deferredAtSend: null,
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

		harnessActivity.recoverUnheardSilence(id);
		expect(forwarded()).toContainEqual({ harnessId: id, input: { kind: "recoverUnheardSilence" } });

		// The daemon's update restores authority; the launch gate re-arms.
		mirrorSet(id, "waiting", { ...live, lastOutputAt: Date.now() });
		vi.advanceTimersByTime(LAUNCH_QUIET_MS);

		expect(sendPrompt(id, "opencode", "you have mail")).toEqual({ ok: true });
		expect(target.paste).toHaveBeenCalledTimes(2);
	});
});
