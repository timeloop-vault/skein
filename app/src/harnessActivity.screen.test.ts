import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { harnessInput } from "./harnessInput.ts";
import type { ScreenPhase } from "./screenPhase.ts";
import { SCREEN_UNKNOWN_RELEASE_TICKS, setScreenPhaseReader } from "./screenPhaseTick.ts";

// #623 — the screen-phase tick rule. Fake timers are file-wide for the same
// reason as harnessActivity.watchdog.test.ts (the tick interval is created
// once, by the first `spawned()`).
beforeAll(() => {
	vi.useFakeTimers();
});
afterAll(() => {
	vi.useRealTimers();
	setScreenPhaseReader(null);
});

const nextId = (() => {
	let n = 0;
	return () => `s_${++n}`;
})();

let reads: Record<string, ScreenPhase> = {};
beforeEach(() => {
	reads = {};
	setScreenPhaseReader((id) => reads[id] ?? "unknown");
});

const tick = (n = 1) => vi.advanceTimersByTime(1000 * n);

const spawnRunning = (): string => {
	const id = nextId();
	harnessActivity.spawned(id);
	harnessActivity.recordOutput(id, "banner");
	expect(harnessActivity.get(id)?.phase).toBe("running");
	return id;
};

describe("screen phase tick", () => {
	it("needs two waiting reads before running -> waiting", () => {
		const id = spawnRunning();
		const sources: string[] = [];
		harnessActivity.subscribeTransitions((tid, _f, _t, s) => {
			if (tid === id) sources.push(s);
		});
		reads[id] = "waiting";
		tick();
		expect(harnessActivity.get(id)?.phase).toBe("running");
		tick();
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(sources).toEqual([TRANSITION_SOURCE.L2bScreen]);
	});

	it("a single torn read does not count", () => {
		const id = spawnRunning();
		reads[id] = "waiting";
		tick();
		reads[id] = "running";
		tick();
		reads[id] = "waiting";
		tick();
		expect(harnessActivity.get(id)?.phase).toBe("running");
	});

	it("stays waiting while output keeps arriving", () => {
		const id = spawnRunning();
		reads[id] = "waiting";
		tick(2);
		for (let i = 0; i < 5; i++) {
			harnessActivity.recordOutput(id, `clock redraw 12:0${i}`);
			tick();
		}
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
	});

	it("waiting -> running after two running reads", () => {
		const id = spawnRunning();
		reads[id] = "waiting";
		tick(2);
		reads[id] = "running";
		tick();
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		tick();
		expect(harnessActivity.get(id)?.phase).toBe("running");
	});

	it("unknown reads leave L2b/L2a behaviour as before", () => {
		const id = spawnRunning();
		harnessActivity.recordOutput(id, "\nPassword: ");
		tick(2);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		// Not screen-set, so output that drains the prompt still wakes it.
		harnessActivity.recordOutput(id, `\n${"welcome ".repeat(60)}`);
		expect(harnessActivity.get(id)?.phase).toBe("running");
		tick(9);
		expect(harnessActivity.get(id)?.phase).toBe("idle");
	});

	it("releases a screen-owned waiting after enough unrecognised ticks", () => {
		const id = spawnRunning();
		reads[id] = "waiting";
		tick(2);
		reads[id] = "unknown";
		tick(SCREEN_UNKNOWN_RELEASE_TICKS - 1);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		tick();
		expect(harnessActivity.get(id)?.phase).toBe("running");
	});

	it("an unknown streak is broken by a recognised read", () => {
		const id = spawnRunning();
		reads[id] = "waiting";
		tick(2);
		reads[id] = "unknown";
		tick(SCREEN_UNKNOWN_RELEASE_TICKS - 1);
		reads[id] = "waiting";
		tick();
		reads[id] = "unknown";
		tick(SCREEN_UNKNOWN_RELEASE_TICKS - 1);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
	});

	it("a submit moves a screen-owned waiting to running at once", () => {
		const id = spawnRunning();
		reads[id] = "waiting";
		tick(2);
		harnessActivity.recordInput(id, "hello");
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		harnessActivity.recordInput(id, "\r");
		expect(harnessActivity.get(id)?.phase).toBe("running");
		harnessActivity.recordOutput(id, "x");
		expect(harnessActivity.get(id)?.phase).toBe("running");
	});

	it("ignores screen reads for an authoritative harness", () => {
		const id = spawnRunning();
		harnessActivity.attachAuthoritativeSource(id);
		reads[id] = "waiting";
		tick(3);
		expect(harnessActivity.get(id)?.phase).toBe("running");
	});

	it("leaves permission and exited alone", () => {
		const p = spawnRunning();
		harnessActivity.setPermissionFromAdapter(p, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		reads[p] = "waiting";
		const e = spawnRunning();
		harnessActivity.exited(e, 0);
		reads[e] = "waiting";
		tick(3);
		expect(harnessActivity.get(p)?.phase).toBe("permission");
		expect(harnessActivity.get(e)?.phase).toBe("exited");
	});
});

describe("readScreenPhaseNow", () => {
	const noop = { paste: () => {}, bracketedPaste: () => false, submit: () => {} };
	it("is unknown when unregistered, unscreened or the snapshot is null", () => {
		expect(harnessInput.readScreenPhaseNow("nobody")).toBe("unknown");
		const un = harnessInput.register("rs1", { ...noop, kind: "hermes" });
		expect(harnessInput.readScreenPhaseNow("rs1")).toBe("unknown");
		un();
		const un2 = harnessInput.register("rs2", { ...noop, kind: "hermes", rawScreen: () => null });
		expect(harnessInput.readScreenPhaseNow("rs2")).toBe("unknown");
		un2();
	});

	it("never snapshots a kind without the screenPhase capability", () => {
		const rawScreen = vi.fn(() => null);
		const un = harnessInput.register("rs3", { ...noop, kind: "claude", rawScreen });
		expect(harnessInput.readScreenPhaseNow("rs3")).toBe("unknown");
		expect(rawScreen).not.toHaveBeenCalled();
		un();
	});
});
