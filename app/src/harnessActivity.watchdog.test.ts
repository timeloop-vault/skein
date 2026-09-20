import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { TRANSITION_SOURCE, harnessActivity } from "./harnessActivity.ts";

// #259 — the silent-adapter watchdog. Its own file because the idle
// tick is a module-global `setInterval` started by the first
// `spawned()`: fake timers have to be installed before that happens,
// and vitest gives each test file a fresh module instance.
//
// Fake timers are installed and torn down ONCE for the whole file
// (file-scope hooks below), not per-`describe`: the tick's
// `setInterval` is created under whichever fake-timer instance is
// live at the moment of the first `spawned()` call, and toggling
// `useFakeTimers`/`useRealTimers` again mid-file (e.g. a second
// `describe` with its own `beforeAll`/`afterAll`) tears down that
// instance and installs a fresh one that never picked up the
// already-running interval — `tickHandle`'s `if (tickHandle !== null)
// return` guard then silently skips creating a new one. Every
// `describe` below (#259's and #273's) shares this one fake-timer
// window.
beforeAll(() => {
	vi.useFakeTimers();
});
afterAll(() => {
	vi.useRealTimers();
});

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

	it("never arms the prompt-gated watchdog on a fresh harness nobody has prompted", () => {
		// Claude writes no transcript until the first prompt, so silence
		// before one is healthy — but a harness sitting silent needs
		// SOME proof of life or #273's own launch-silent timer degrades
		// it after LAUNCH_SILENT_AFTER_MS regardless of a prompt. Give
		// it a launch signal (the CLI reaching its own input prompt) so
		// this test isolates what it's actually about: the
		// ADAPTER_SILENT_AFTER_MS watchdog only arms on an actual
		// prompt (`\r`/`\n`), which "h" alone never sends.
		const id = attachedHarness();
		harnessActivity.noteLaunchSignal(id);
		harnessActivity.recordInput(id, "h");
		vi.advanceTimersByTime(60_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
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
		expect(harnessActivity.get(id)?.degradedBy).toBe("adapter-silent");

		harnessActivity.adapterDelivered(id);
		harnessActivity.setWaitingFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		expect(harnessActivity.get(id)?.authoritative).toBe(true);
		expect(harnessActivity.get(id)?.adapterSilent).toBe(false);
		expect(harnessActivity.get(id)?.degradedBy).toBeNull();

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

// #273: the launch-signal channel (Claude's `SessionStart` hook via
// #215 injection) and the launch-silent fallback timer that covers a
// harness with no such channel at all.
describe("launch signal (#273)", () => {
	it("moves a fresh authoritative harness to waiting and records launchSignalAt", () => {
		const id = attachedHarness();
		harnessActivity.noteLaunchSignal(id);
		const a = harnessActivity.get(id);
		expect(a?.phase).toBe("waiting");
		expect(a?.launchSignalAt).not.toBeNull();
	});

	it("is idempotent across a phantom second SessionStart fire (claude-code#78455)", () => {
		const id = attachedHarness();
		const sources: string[] = [];
		const unsubscribe = harnessActivity.subscribeTransitions((tid, _from, _to, source) => {
			if (tid === id) sources.push(source);
		});
		harnessActivity.noteLaunchSignal(id);
		harnessActivity.noteLaunchSignal(id);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(sources).toEqual([TRANSITION_SOURCE.L2c1ClaudeSessionStart]);
		unsubscribe();
	});

	it("records the signal without moving phase once the tail has already spoken", () => {
		const id = attachedHarness();
		harnessActivity.adapterDelivered(id);
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeAssistant);
		harnessActivity.noteLaunchSignal(id);
		const a = harnessActivity.get(id);
		expect(a?.phase).toBe("running");
		expect(a?.launchSignalAt).not.toBeNull();
	});

	it("leaves permission alone — it outranks a launch signal (#86)", () => {
		const id = attachedHarness();
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		harnessActivity.noteLaunchSignal(id);
		expect(harnessActivity.get(id)?.phase).toBe("permission");
	});

	it("degrades a harness out of spawning after LAUNCH_SILENT_AFTER_MS with no launch signal", () => {
		const id = attachedHarness();
		const sources: string[] = [];
		const unsubscribe = harnessActivity.subscribeTransitions((tid, _from, _to, source) => {
			if (tid === id) sources.push(source);
		});

		vi.advanceTimersByTime(14_000);
		expect(harnessActivity.get(id)?.phase).toBe("spawning");
		expect(harnessActivity.get(id)?.authoritative).toBe(true);

		vi.advanceTimersByTime(2_000);
		expect(harnessActivity.get(id)?.phase).toBe("running");
		expect(harnessActivity.get(id)?.authoritative).toBe(false);
		expect(sources).toEqual([TRANSITION_SOURCE.LaunchSilent]);
		unsubscribe();
	});

	it("restores authority and moves to waiting when a launch signal arrives after the launch-silent degrade", () => {
		const id = attachedHarness();
		vi.advanceTimersByTime(16_000);
		expect(harnessActivity.get(id)?.authoritative).toBe(false);
		expect(harnessActivity.get(id)?.degradedBy).toBe("launch-silent");

		harnessActivity.noteLaunchSignal(id);
		expect(harnessActivity.get(id)?.authoritative).toBe(true);
		expect(harnessActivity.get(id)?.adapterSilent).toBe(false);
		expect(harnessActivity.get(id)?.degradedBy).toBeNull();
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
	});

	it("does not degrade a harness that already has a launch signal", () => {
		const id = attachedHarness();
		harnessActivity.noteLaunchSignal(id);
		vi.advanceTimersByTime(20_000);
		expect(harnessActivity.get(id)?.authoritative).toBe(true);
		expect(harnessActivity.get(id)?.adapterSilent).toBe(false);
	});

	// Regression: a launch signal must NOT recover a harness the #259
	// watchdog gave up on. The two watchdogs diagnose different things —
	// #259 means "a PROMPTED tail stayed mute" (the tail is probably
	// reading the wrong file), #273's own timer means "nothing has
	// proven this harness alive yet" — and a `SessionStart` hook firing
	// only speaks to the second. Before the fix, `noteLaunchSignal`
	// treated any `adapterSilent` harness as recoverable and this test
	// failed: `authoritative` came back `true` and `phase` moved to
	// `waiting` even though the tail had never actually spoken.
	it("does NOT recover a harness the #259 adapter-silent watchdog gave up on", () => {
		const id = attachedHarness();
		harnessActivity.recordInput(id, "\r");
		vi.advanceTimersByTime(11_000);
		expect(harnessActivity.get(id)?.phase).toBe("running");
		expect(harnessActivity.get(id)?.adapterSilent).toBe(true);
		expect(harnessActivity.get(id)?.degradedBy).toBe("adapter-silent");

		// The user later types `/clear`; the matcher-less SessionStart
		// hook fires again even though the tail is still reading the
		// wrong file.
		harnessActivity.noteLaunchSignal(id);

		expect(harnessActivity.get(id)?.authoritative).toBe(false);
		expect(harnessActivity.get(id)?.adapterSilent).toBe(true);
		expect(harnessActivity.get(id)?.degradedBy).toBe("adapter-silent");
		expect(harnessActivity.get(id)?.phase).not.toBe("waiting");
		expect(harnessActivity.get(id)?.phase).toBe("running");
		// The fact itself is still true and worth recording.
		expect(harnessActivity.get(id)?.launchSignalAt).not.toBeNull();
	});

	// #273's own timer must never degrade a harness whose adapter has
	// already proven itself — the case that keeps a fresh opencode
	// harness (whose SSE reports `connected` well before
	// LAUNCH_SILENT_AFTER_MS) out of this path entirely.
	it("never degrades an authoritative harness that the adapter confirms before the launch-silent timer", () => {
		const id = attachedHarness();
		harnessActivity.adapterDelivered(id);
		vi.advanceTimersByTime(20_000);
		expect(harnessActivity.get(id)?.authoritative).toBe(true);
		expect(harnessActivity.get(id)?.adapterSilent).toBe(false);
		expect(harnessActivity.get(id)?.degradedBy).toBeNull();
	});
});
