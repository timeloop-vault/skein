import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { TRANSITION_SOURCE, harnessActivity } from "./harnessActivity.ts";
import { ADAPTER_SILENT_AFTER_MS } from "./harnessActivityConstants.ts";
import { harnessInput, sendPrompt } from "./harnessInput.ts";
import type { HarnessInputTarget } from "./harnessInput.ts";

// #363 — `sendPrompt` (the #238/#327/#330 seam: nudges, mail delivery,
// create_room's first prompt) must arm the #259 silent-adapter watchdog
// the same way a typed Enter does, since none of those callers ever
// touch xterm's `onKey`. Fake timers are installed at file scope, not
// per-test, for the same reason `harnessActivity.watchdog.test.ts`
// does it that way: the tick's `setInterval` is created by the first
// `harnessActivity.spawned()` call, under whichever fake-timer instance
// is live at that moment.
beforeAll(() => {
	vi.useFakeTimers();
});
afterAll(() => {
	vi.useRealTimers();
});

const nextId = (() => {
	let n = 0;
	return () => `si_${++n}`;
})();

/// A registered, sendable harness whose only proof of life is the
/// launch signal (#273) — `adapterHeard` stays false, which is exactly
/// what `shouldArmWatchdog` needs to see to arm on a submit.
const sendableHarness = (): { id: string; target: HarnessInputTarget } => {
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
	return { id, target };
};

describe("sendPrompt arms the #259 watchdog (#363)", () => {
	it("degrades a seam-submitted prompt exactly like a typed Enter, when the adapter never speaks", () => {
		const { id } = sendableHarness();
		expect(harnessActivity.get(id)?.promptSubmittedAt).toBeNull();

		const result = sendPrompt(id, "claude", "do the thing");
		expect(result).toEqual({ ok: true });
		expect(harnessActivity.get(id)?.promptSubmittedAt).not.toBeNull();

		vi.advanceTimersByTime(ADAPTER_SILENT_AFTER_MS + 1_000);

		const a = harnessActivity.get(id);
		expect(a?.adapterSilent).toBe(true);
		expect(a?.degradedBy).toBe("adapter-silent");
		expect(a?.authoritative).toBe(false);
	});

	it("refuses — and never pastes or submits, or arms anything — a harness stuck on a permission dialog", () => {
		const { id, target } = sendableHarness();
		harnessActivity.setPermissionFromAdapter(
			id,
			TRANSITION_SOURCE.L2c1ClaudePermission,
			"Bash",
			null,
			"agent-1",
		);

		harnessActivity.notePromptSubmitted(id);
		const a = harnessActivity.get(id);
		expect(a?.phase).toBe("permission");
		expect(a?.permissionAgentId).toBe("agent-1");
		expect(a?.promptSubmittedAt).toBeNull();

		const result = sendPrompt(id, "claude", "anything");
		expect(result.ok).toBe(false);
		expect(target.paste).not.toHaveBeenCalled();
		expect(target.submit).not.toHaveBeenCalled();
	});
});

describe("harnessActivity.notePromptSubmitted", () => {
	it("never flips hasUserInput", () => {
		const { id } = sendableHarness();
		expect(harnessActivity.get(id)?.hasUserInput).toBe(false);
		harnessActivity.notePromptSubmitted(id);
		expect(harnessActivity.get(id)?.hasUserInput).toBe(false);
	});

	it("is a no-op once the adapter has already spoken", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		harnessActivity.adapterDelivered(id);
		harnessActivity.notePromptSubmitted(id);
		expect(harnessActivity.get(id)?.promptSubmittedAt).toBeNull();
	});

	it("does not overwrite an already-armed timer", () => {
		const { id } = sendableHarness();
		harnessActivity.notePromptSubmitted(id);
		const first = harnessActivity.get(id)?.promptSubmittedAt ?? null;
		expect(first).not.toBeNull();

		vi.advanceTimersByTime(500);
		harnessActivity.notePromptSubmitted(id);
		expect(harnessActivity.get(id)?.promptSubmittedAt).toBe(first);
	});
});
