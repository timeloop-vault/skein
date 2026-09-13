import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { TRANSITION_SOURCE, harnessActivity } from "./harnessActivity.ts";

// #259 — the silent-adapter watchdog. Its own file because the idle
// tick is a module-global `setInterval` started by the first
// `spawned()`: fake timers have to be installed before that happens,
// and vitest gives each test file a fresh module instance.

const nextId = (() => {
	let n = 0;
	return () => `w_${++n}`;
})();

/// A Claude harness as LiveTerminal leaves it: spawned, adapter
/// attached, TUI output flowing (which authority keeps from moving
/// the phase).
const attachedHarness = (): string => {
	const id = nextId();
	harnessActivity.spawned(id);
	harnessActivity.attachAuthoritativeSource(id);
	harnessActivity.recordOutput(id, "welcome banner");
	return id;
};

describe("silent-adapter watchdog", () => {
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});

	it("hands a harness back to L2a when the adapter stays silent after a prompt", () => {
		const id = attachedHarness();
		const sources: string[] = [];
		const unsubscribe = harnessActivity.subscribeTransitions((tid, _from, _to, source) => {
			if (tid === id) sources.push(source);
		});

		harnessActivity.recordInput(id, "\r");
		vi.advanceTimersByTime(9_000);
		expect(harnessActivity.get(id)?.phase).toBe("spawning");
		expect(harnessActivity.get(id)?.authoritative).toBe(true);

		vi.advanceTimersByTime(2_000);
		expect(harnessActivity.get(id)?.phase).toBe("running");
		expect(harnessActivity.get(id)?.authoritative).toBe(false);
		expect(sources).toEqual([TRANSITION_SOURCE.AdapterSilent]);

		// And L2a actually runs it from here: quiet → idle.
		vi.advanceTimersByTime(9_000);
		expect(harnessActivity.get(id)?.phase).toBe("idle");
		unsubscribe();
	});

	it("never fires on a fresh harness nobody has prompted", () => {
		// Claude writes no transcript until the first prompt, so silence
		// before one is healthy.
		const id = attachedHarness();
		harnessActivity.recordInput(id, "h");
		vi.advanceTimersByTime(60_000);
		expect(harnessActivity.get(id)?.phase).toBe("spawning");
		expect(harnessActivity.get(id)?.authoritative).toBe(true);
	});

	it("stands down once the adapter has delivered", () => {
		const id = attachedHarness();
		harnessActivity.adapterDelivered(id);
		harnessActivity.recordInput(id, "\r");
		vi.advanceTimersByTime(60_000);
		expect(harnessActivity.get(id)?.authoritative).toBe(true);
	});

	it("gives authority back when a silenced adapter speaks after all", () => {
		const id = attachedHarness();
		harnessActivity.recordInput(id, "\r");
		vi.advanceTimersByTime(11_000);
		expect(harnessActivity.get(id)?.authoritative).toBe(false);

		harnessActivity.adapterDelivered(id);
		harnessActivity.setWaitingFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		expect(harnessActivity.get(id)?.authoritative).toBe(true);
		expect(harnessActivity.get(id)?.adapterSilent).toBe(false);

		// Authoritative again, so L2a leaves the adapter's phase alone.
		vi.advanceTimersByTime(60_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
	});

	it("does not watch harnesses without an adapter", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.recordInput(id, "\r");
		expect(harnessActivity.get(id)?.promptSubmittedAt).toBeNull();
	});
});
