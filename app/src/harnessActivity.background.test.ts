import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

// #446 / #441 — background tasks hold a main session's end of turn like
// subagents do, and a running harness with outstanding work but no signal
// is presumed stale. Driven through the real `attachClaudeEvents`
// translator on a mocked Channel. Own file: the idle tick is a
// module-global interval, so fake timers must precede the first spawned().

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
import { atSafeStoppingPoint, harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import {
	BACKGROUND_TASK_CEILING_MS,
	DELEGATION_CEILING_MS,
	DELEGATION_SETTLE_MS,
} from "./harnessActivityConstants.ts";
import { attachClaudeEvents } from "./harnessEvents.ts";
import { shouldCheckOnTransition } from "./mailNudge.ts";
import { subagents } from "./subagents.ts";

const MIN = 60_000;
let n = 0;
const made: string[] = [];
const send = (event: unknown) => h.channels[h.channels.length - 1]?.onmessage(event);

/// Claude harness mid-turn with its tail attached (the last-attached
/// channel is this harness's, so build and drive one at a time).
const attached = (): string => {
	const id = `bg_${++n}`;
	made.push(id);
	harnessActivity.spawned(id);
	attachClaudeEvents(id, "room", "sid", "/cwd");
	send({ kind: "assistant_turn" });
	return id;
};

const startTask = (over: Record<string, unknown> = {}, taskId = "t1"): void =>
	send({
		kind: "background_start",
		task_id: taskId,
		tool_use_id: `tu_${taskId}`,
		task_kind: "bash",
		description: null,
		command: "sleep 999",
		timeout_ms: null,
		persistent: false,
		auto_backgrounded: false,
		agent_id: null,
		initial: false,
		...over,
	});
const endTask = (taskId = "t1"): void =>
	send({
		kind: "background_end",
		task_id: taskId,
		task_kind: "bash",
		agent_id: null,
		status: "completed",
		exit_code: 0,
	});
const endTurn = (): void => send({ kind: "awaiting_prompt" });
const prompt = (): void => send({ kind: "user_prompt" });
const startSub = (): void =>
	send({
		kind: "subagent_start",
		agent_id: "a1",
		agent_type: "explore",
		description: null,
		initial: false,
	});
const endSub = (): void =>
	send({ kind: "subagent_end", agent_id: "a1", agent_type: "explore", description: null });

/// atSafeStoppingPoint of the live record (false when gone).
const safeNow = (id: string): boolean => {
	const a = harnessActivity.get(id);
	return a ? atSafeStoppingPoint(a) : false;
};
const phase = (id: string) => harnessActivity.get(id)?.phase;
/// Record transitions for one harness; returns the live array.
const watch = (id: string) => {
	const t: Array<{ to: string; source: string }> = [];
	const off = harnessActivity.subscribeTransitions((tid, _f, to, source) => {
		if (tid === id) t.push({ to, source });
	});
	unsubs.push(off);
	return t;
};
const unsubs: Array<() => void> = [];

describe("background-task deferral + stale-work watchdog (#446, #441)", () => {
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

	it("1a: outstanding Bash task defers; end + prompt + next end of turn -> waiting (end-turn)", () => {
		const id = attached();
		startTask();
		const t = watch(id);
		endTurn();
		expect(phase(id)).toBe("running");
		expect(t).toEqual([]);
		endTask();
		prompt();
		endTurn();
		expect(phase(id)).toBe("waiting");
		expect(t[t.length - 1]).toEqual({ to: "waiting", source: TRANSITION_SOURCE.L2c1ClaudeEndTurn });
	});

	it("1b: background_end alone then silence -> waiting via delegation-settled", () => {
		const id = attached();
		startTask();
		endTurn();
		endTask();
		const t = watch(id);
		vi.advanceTimersByTime(DELEGATION_SETTLE_MS - 3_000);
		expect(phase(id)).toBe("running");
		vi.advanceTimersByTime(5_000);
		expect(phase(id)).toBe("waiting");
		expect(t).toEqual([{ to: "waiting", source: TRANSITION_SOURCE.DelegationSettled }]);
	});

	it("2: Monitor with a deadline expires from the registry, then settles", () => {
		const id = attached();
		startTask({ task_kind: "monitor", timeout_ms: 60_000, persistent: false });
		endTurn();
		const t = watch(id);
		vi.advanceTimersByTime(50_000);
		expect(phase(id)).toBe("running");
		expect(backgroundTasks.live(id)).toHaveLength(1);
		vi.advanceTimersByTime(12_000);
		expect(backgroundTasks.live(id)).toHaveLength(0);
		expect(phase(id)).toBe("running");
		vi.advanceTimersByTime(DELEGATION_SETTLE_MS + 3_000);
		expect(phase(id)).toBe("waiting");
		expect(t).toEqual([{ to: "waiting", source: TRANSITION_SOURCE.DelegationSettled }]);
	});

	it("3: Bash ceiling flushes with delegation-ceiling, task stays overdue, later turns don't re-defer", () => {
		const id = attached();
		startTask();
		endTurn();
		const t = watch(id);
		vi.advanceTimersByTime(BACKGROUND_TASK_CEILING_MS - 3_000);
		expect(phase(id)).toBe("running");
		expect(t).toEqual([]);
		vi.advanceTimersByTime(5_000);
		expect(phase(id)).toBe("waiting");
		expect(t).toEqual([{ to: "waiting", source: TRANSITION_SOURCE.DelegationCeiling }]);
		expect(backgroundTasks.live(id)).toHaveLength(1);
		expect(backgroundTasks.live(id)[0]?.overdue).toBe(true);
		expect(backgroundTasks.workingCount(id)).toBe(0);
		expect(backgroundTasks.overdueCount(id)).toBe(1);

		prompt();
		endTurn();
		expect(phase(id)).toBe("waiting");
		expect(harnessActivity.get(id)?.delegationDeferredAt).toBeNull();
	});

	it("3b: a ceiling flush resets delegatedCount so a later waiting claims nothing finished", () => {
		const id = attached();
		startSub();
		endSub();
		startTask();
		endTurn();
		expect(harnessActivity.get(id)?.delegatedCount).toBe(1);
		vi.advanceTimersByTime(BACKGROUND_TASK_CEILING_MS + 5_000);
		expect(phase(id)).toBe("waiting");
		expect(harnessActivity.get(id)?.delegatedCount).toBe(0);
	});

	it("4: a persistent Monitor with a timeout uses the Bash ceiling, not its deadline", () => {
		const id = attached();
		startTask({ task_kind: "monitor", timeout_ms: 60_000, persistent: true });
		endTurn();
		const t = watch(id);
		vi.advanceTimersByTime(5 * MIN);
		expect(phase(id)).toBe("running");
		expect(backgroundTasks.live(id)).toHaveLength(1);
		vi.advanceTimersByTime(BACKGROUND_TASK_CEILING_MS - 5 * MIN + 3_000);
		expect(phase(id)).toBe("waiting");
		expect(t).toEqual([{ to: "waiting", source: TRANSITION_SOURCE.DelegationCeiling }]);
	});

	it("5: attach-seeded tasks never defer", () => {
		const id = attached();
		startTask({ initial: true });
		expect(backgroundTasks.live(id)).toHaveLength(1);
		expect(backgroundTasks.workingCount(id)).toBe(0);
		const t = watch(id);
		endTurn();
		expect(phase(id)).toBe("waiting");
		expect(t).toEqual([{ to: "waiting", source: TRANSITION_SOURCE.L2c1ClaudeEndTurn }]);
	});

	it("6a: subagent + task count together; subagent_end alone keeps it deferred", () => {
		const id = attached();
		startSub();
		startTask();
		endTurn();
		endSub();
		const t = watch(id);
		vi.advanceTimersByTime(5 * MIN);
		expect(phase(id)).toBe("running");
		expect(t).toEqual([]);
		endTask();
		vi.advanceTimersByTime(DELEGATION_SETTLE_MS + 3_000);
		expect(phase(id)).toBe("waiting");
		expect(t).toEqual([{ to: "waiting", source: TRANSITION_SOURCE.DelegationSettled }]);
	});

	it("6b: subagent ceiling alone does not flush while a task is within its ceiling; flush once both released", () => {
		const id = attached();
		startSub(); // t0
		vi.advanceTimersByTime(10 * MIN);
		startTask(); // t0+10m; arming happens now, so its ceiling is t0+25m
		endTurn();
		const t = watch(id);
		vi.advanceTimersByTime(DELEGATION_CEILING_MS - 10 * MIN + 5_000); // t0+15m: subagent presumed gone
		expect(subagents.workingCount(id)).toBe(0);
		expect(phase(id)).toBe("running");
		expect(t).toEqual([]);
		vi.advanceTimersByTime(10 * MIN + 5_000); // past the task's ceiling
		expect(phase(id)).toBe("waiting");
		expect(t).toEqual([{ to: "waiting", source: TRANSITION_SOURCE.DelegationCeiling }]);
	});

	it("7a: #441 watchdog flushes running-with-work after a silent ceiling", () => {
		const id = attached();
		startSub();
		startTask();
		const t = watch(id);
		vi.advanceTimersByTime(DELEGATION_CEILING_MS - 5_000);
		expect(phase(id)).toBe("running");
		expect(t).toEqual([]);
		vi.advanceTimersByTime(10_000);
		expect(phase(id)).toBe("waiting");
		expect(t).toEqual([{ to: "waiting", source: TRANSITION_SOURCE.WorkWatchdog }]);
		expect(backgroundTasks.live(id)[0]?.overdue).toBe(true);
		expect(backgroundTasks.overdueCount(id)).toBe(1);
		expect(subagents.workingCount(id)).toBe(0);
	});

	it("7b: fresh main-transcript work keeps resetting the watchdog", () => {
		const id = attached();
		startTask();
		for (let i = 0; i < 4; i++) {
			vi.advanceTimersByTime(10 * MIN);
			send({ kind: "tool_use_start", name: "Read" });
		}
		expect(phase(id)).toBe("running");
		expect(backgroundTasks.workingCount(id)).toBe(1);
		vi.advanceTimersByTime(DELEGATION_CEILING_MS + 5_000);
		expect(phase(id)).toBe("waiting");
	});

	it("8a: stale-state release hands held mail a safe-stopping-point transition", () => {
		const id = attached();
		startTask();
		expect(safeNow(id)).toBe(false);
		const seen: Array<{ to: string; safe: boolean; check: boolean }> = [];
		unsubs.push(
			harnessActivity.subscribeTransitions((tid, _f, to) => {
				if (tid !== id) return;
				const safe = safeNow(id);
				seen.push({ to, safe, check: shouldCheckOnTransition(to, safe) });
			}),
		);
		vi.advanceTimersByTime(DELEGATION_CEILING_MS - 5_000);
		expect(safeNow(id)).toBe(false);
		vi.advanceTimersByTime(10_000);
		expect(seen).toEqual([{ to: "waiting", safe: true, check: true }]);
	});

	it("8b: Bash-ceiling release hands held mail a safe-stopping-point transition", () => {
		const id = attached();
		startTask();
		endTurn();
		expect(safeNow(id)).toBe(true); // deferred
		const seen: Array<{ to: string; safe: boolean; check: boolean }> = [];
		unsubs.push(
			harnessActivity.subscribeTransitions((tid, _f, to) => {
				if (tid !== id) return;
				const safe = safeNow(id);
				seen.push({ to, safe, check: shouldCheckOnTransition(to, safe) });
			}),
		);
		vi.advanceTimersByTime(BACKGROUND_TASK_CEILING_MS + 5_000);
		expect(seen).toEqual([{ to: "waiting", safe: true, check: true }]);
	});
});
