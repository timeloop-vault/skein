import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

// Epic #439 field report: a Claude harness armed a `Monitor` (30 min
// timeout, not persistent), then ~16 times in 2 minutes a task-notification
// user row (a Monitor event — no terminal status) was followed seconds later
// by a text-only end of turn. Without background tracking each end of turn
// flipped the phase to `waiting` and the next notification back to `running`.
// Driven through the real `attachClaudeEvents` translator on a mocked
// Channel. Own file: the idle tick is a module-global interval, so fake
// timers must precede the first spawned().

const h = vi.hoisted(() => {
	const channels: Array<{ onmessage: (e: unknown) => void }> = [];
	return { channels };
});

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
		constructor() {
			h.channels.push(this);
		}
	},
	invoke: () => new Promise<void>(() => {}),
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { backgroundTasks } from "./backgroundTasks.ts";
import { harnessActivity, statusLabel, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { DELEGATION_SETTLE_MS } from "./harnessActivityConstants.ts";
import { attachClaudeEvents } from "./harnessEvents.ts";
import { subagents } from "./subagents.ts";

const MIN = 60_000;
const REPETITIONS = 16;
const MONITOR_TIMEOUT_MS = 30 * MIN;
let n = 0;
const made: string[] = [];
const unsubs: Array<() => void> = [];
const send = (event: unknown) => h.channels[h.channels.length - 1]?.onmessage(event);
const phase = (id: string) => harnessActivity.get(id)?.phase;

const watch = (id: string) => {
	const t: Array<{ to: string; source: string }> = [];
	unsubs.push(
		harnessActivity.subscribeTransitions((tid, _f, to, source) => {
			if (tid === id) t.push({ to, source });
		}),
	);
	return t;
};

const label = (id: string): string =>
	statusLabel("running", null, null, subagents.workingCount(id), backgroundTasks.workingCount(id));

/// Claude harness mid-turn that has just armed a live 30-minute Monitor and
/// ended its turn (the deferral is now armed).
const monitorArmed = (): string => {
	const id = `mf_${++n}`;
	made.push(id);
	harnessActivity.spawned(id);
	attachClaudeEvents(id, "room", "sid", "/cwd");
	send({ kind: "assistant_turn" });
	send({
		kind: "background_start",
		task_id: "mon1",
		tool_use_id: "tu_mon1",
		task_kind: "monitor",
		description: "watch the build",
		command: "tail -f build.log",
		timeout_ms: MONITOR_TIMEOUT_MS,
		persistent: false,
		auto_backgrounded: false,
		agent_id: null,
		initial: false,
	});
	send({ kind: "awaiting_prompt" });
	return id;
};

/// One Monitor event: a task-notification user row, some assistant work,
/// then a text-only end of turn a few seconds later.
const monitorEvent = (): void => {
	send({ kind: "user_prompt", task_notification: true });
	send({ kind: "assistant_turn" });
	vi.advanceTimersByTime(3_000);
	send({ kind: "awaiting_prompt" });
};

describe("Monitor event pattern does not flicker the phase (epic #439)", () => {
	beforeAll(() => {
		vi.useFakeTimers();
		vi.spyOn(console, "warn").mockImplementation(() => {});
	});
	afterEach(() => {
		for (const off of unsubs.splice(0)) off();
		for (const id of made.splice(0)) {
			harnessActivity.forget(id);
			backgroundTasks.forget(id);
			subagents.forget(id);
		}
	});

	it("stays running with the background label through every notification + end of turn", () => {
		const id = monitorArmed();
		const t = watch(id);
		for (let i = 0; i < REPETITIONS; i++) {
			vi.advanceTimersByTime(4_000);
			monitorEvent();
			expect(phase(id), `repetition ${i}`).toBe("running");
			expect(label(id), `repetition ${i}`).toBe("background · 1 task");
		}
		// No `waiting` was ever entered, so nothing notified (toasts and OS
		// banners hang off the transition to waiting).
		expect(t.filter((x) => x.to === "waiting")).toEqual([]);
		expect(backgroundTasks.workingCount(id)).toBe(1);
	});

	it("moves to waiting exactly once after the Monitor's deadline", () => {
		const id = monitorArmed();
		const startedAt = Date.now();
		const t = watch(id);
		for (let i = 0; i < REPETITIONS; i++) {
			vi.advanceTimersByTime(4_000);
			monitorEvent();
		}
		expect(t.filter((x) => x.to === "waiting")).toEqual([]);

		// Just before the deadline: still deferred.
		vi.advanceTimersByTime(startedAt + MONITOR_TIMEOUT_MS - Date.now() - 5_000);
		expect(phase(id)).toBe("running");
		expect(backgroundTasks.workingCount(id)).toBe(1);

		// Past the deadline the registry drops it, then the ordinary settle
		// rule flushes once; nothing further follows.
		vi.advanceTimersByTime(10_000);
		expect(backgroundTasks.workingCount(id)).toBe(0);
		vi.advanceTimersByTime(DELEGATION_SETTLE_MS + 5_000);
		expect(phase(id)).toBe("waiting");
		vi.advanceTimersByTime(10 * MIN);
		expect(t.filter((x) => x.to === "waiting")).toEqual([
			{ to: "waiting", source: TRANSITION_SOURCE.DelegationSettled },
		]);
	});
});
