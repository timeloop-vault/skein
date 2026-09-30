import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type BackgroundTaskInput, backgroundTasks, deadlineOf } from "./backgroundTasks.ts";

// #446 — per-harness live background-task bookkeeping, pure.

const H = "h_bg";

const task = (taskId: string, over: Partial<BackgroundTaskInput> = {}): BackgroundTaskInput => ({
	taskId,
	kind: "bash",
	description: null,
	command: null,
	timeoutMs: null,
	persistent: false,
	agentId: null,
	...over,
});

beforeEach(() => {
	vi.useFakeTimers();
	vi.setSystemTime(1_000);
});

afterEach(() => {
	backgroundTasks.forget(H);
	vi.useRealTimers();
});

describe("backgroundTasks", () => {
	it("record is idempotent and preserves the original startedAt", () => {
		backgroundTasks.record(H, task("t1"), false);
		vi.setSystemTime(5_000);
		backgroundTasks.record(H, task("t1"), false);
		expect(backgroundTasks.live(H)).toHaveLength(1);
		expect(backgroundTasks.live(H)[0]?.startedAt).toBe(1_000);
	});

	it("an attach-only entry is listed but not working; a live re-record promotes it; replay never demotes", () => {
		backgroundTasks.record(H, task("t1"), true);
		expect(backgroundTasks.live(H)).toHaveLength(1);
		expect(backgroundTasks.workingCount(H)).toBe(0);
		backgroundTasks.record(H, task("t1"), false);
		expect(backgroundTasks.workingCount(H)).toBe(1);
		backgroundTasks.record(H, task("t1"), true);
		expect(backgroundTasks.workingCount(H)).toBe(1);
		expect(backgroundTasks.live(H)[0]?.fromAttach).toBe(false);
	});

	it("live is oldest first and referentially stable without changes", () => {
		backgroundTasks.record(H, task("late"), false);
		vi.setSystemTime(500);
		backgroundTasks.record(H, task("early"), false);
		const a = backgroundTasks.live(H);
		expect(a.map((e) => e.taskId)).toEqual(["early", "late"]);
		expect(backgroundTasks.live(H)).toBe(a);
	});

	it("finish removes; unknown id is a no-op; forget clears", () => {
		backgroundTasks.record(H, task("t1"), false);
		backgroundTasks.record(H, task("t2"), false);
		backgroundTasks.finish(H, "t1");
		expect(backgroundTasks.live(H).map((e) => e.taskId)).toEqual(["t2"]);
		expect(() => backgroundTasks.finish(H, "nope")).not.toThrow();
		backgroundTasks.forget(H);
		expect(backgroundTasks.live(H)).toHaveLength(0);
		expect(() => backgroundTasks.forget(H)).not.toThrow();
	});

	it("deadlineOf only gives a non-persistent Monitor with a timeout a deadline", () => {
		const base = { startedAt: 100, persistent: false, timeoutMs: 50 };
		expect(deadlineOf({ ...base, kind: "bash" })).toBeNull();
		expect(deadlineOf({ ...base, kind: "powershell" })).toBeNull();
		expect(deadlineOf({ ...base, kind: "monitor" })).toBe(150);
		expect(deadlineOf({ ...base, kind: "monitor", persistent: true })).toBeNull();
		expect(deadlineOf({ ...base, kind: "monitor", timeoutMs: null })).toBeNull();
	});

	it("expireDue removes only due monitors and returns the working count removed", () => {
		backgroundTasks.record(H, task("due", { kind: "monitor", timeoutMs: 1_000 }), false);
		backgroundTasks.record(H, task("later", { kind: "monitor", timeoutMs: 9_000 }), false);
		backgroundTasks.record(H, task("bash"), false);
		backgroundTasks.record(H, task("orphan", { kind: "monitor", timeoutMs: 100 }), true);
		expect(backgroundTasks.expireDue(H, 1_500)).toBe(0);
		expect(backgroundTasks.expireDue(H, 2_000)).toBe(1);
		expect(
			backgroundTasks
				.live(H)
				.map((e) => e.taskId)
				.sort(),
		).toEqual(["bash", "later"]);
	});

	it("markOverdue keeps the entry but drops it from workingCount", () => {
		backgroundTasks.record(H, task("t1"), false);
		backgroundTasks.record(H, task("t2"), false);
		backgroundTasks.record(H, task("o"), true);
		expect(backgroundTasks.markOverdue(H, ["t1", "o", "missing"])).toBe(1);
		expect(backgroundTasks.markOverdue(H, ["t1"])).toBe(0);
		expect(backgroundTasks.live(H)).toHaveLength(3);
		expect(backgroundTasks.workingCount(H)).toBe(1);
		expect(backgroundTasks.overdueCount(H)).toBe(1);
	});

	it("overdue is sticky across a re-record", () => {
		backgroundTasks.record(H, task("t1"), false);
		backgroundTasks.markOverdue(H, ["t1"]);
		backgroundTasks.record(H, task("t1"), false);
		expect(backgroundTasks.live(H)[0]?.overdue).toBe(true);
		expect(backgroundTasks.workingCount(H)).toBe(0);
	});

	it("presumeGone marks working entries overdue without deleting", () => {
		backgroundTasks.record(H, task("a"), false);
		backgroundTasks.record(H, task("b"), false);
		backgroundTasks.record(H, task("o"), true);
		expect(backgroundTasks.presumeGone(H)).toBe(2);
		expect(backgroundTasks.presumeGone(H)).toBe(0);
		expect(backgroundTasks.live(H)).toHaveLength(3);
		expect(backgroundTasks.workingCount(H)).toBe(0);
		expect(backgroundTasks.overdueCount(H)).toBe(2);
	});

	it("subscribe fires on changes, not on no-ops, and stops after unsubscribe", () => {
		const cb = vi.fn();
		const off = backgroundTasks.subscribe(H, cb);
		backgroundTasks.record(H, task("t1"), false); // 1
		backgroundTasks.finish(H, "nope"); // no-op
		backgroundTasks.markOverdue(H, ["nope"]); // no-op
		expect(backgroundTasks.expireDue(H, 99_999)).toBe(0); // no-op
		backgroundTasks.presumeGone(H); // 2
		backgroundTasks.presumeGone(H); // no-op
		backgroundTasks.finish(H, "t1"); // 3
		expect(cb).toHaveBeenCalledTimes(3);
		off();
		backgroundTasks.record(H, task("t2"), false);
		expect(cb).toHaveBeenCalledTimes(3);
	});
});
