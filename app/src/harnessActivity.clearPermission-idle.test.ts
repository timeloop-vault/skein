import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { TRANSITION_SOURCE, harnessActivity } from "./harnessActivity.ts";

// #298 — `clearPermission` must be a no-op from `idle`, same as the
// other non-`permission` phases covered in `harnessActivity.test.ts`.
// Own file because reaching a real `idle` phase needs the L2a tick,
// which is a module-global `setInterval` started by the first
// `spawned()` — fake timers have to be installed before that happens,
// same reason `harnessActivity.watchdog.test.ts` (#259) is its own
// file, and vitest gives each test file a fresh module instance.

const nextId = (() => {
	let n = 0;
	return () => `ci_${++n}`;
})();

describe("clearPermission (#298): no-op from idle", () => {
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});

	it("does not move idle to running", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		// Non-authoritative harness: one chunk of output moves
		// spawning -> running, then silence past IDLE_AFTER_MS (8s)
		// lets the L2a tick flip it to idle.
		harnessActivity.recordOutput(id, "hello\n");
		expect(harnessActivity.get(id)?.phase).toBe("running");
		vi.advanceTimersByTime(9_000);
		expect(harnessActivity.get(id)?.phase).toBe("idle");

		harnessActivity.clearPermission(id, TRANSITION_SOURCE.L2c1ClaudeSubagentToolResult);
		expect(harnessActivity.get(id)?.phase).toBe("idle");
	});
});
