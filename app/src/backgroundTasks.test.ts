import { beforeEach, describe, expect, it } from "vitest";
import { backgroundTasks, deadlineOf } from "./backgroundTasks.ts";
import { resetPhaseMirror } from "./phaseMirror.ts";
import { mirrorForget, mirrorSet, taskEntry } from "./phaseTestSupport.ts";

// #732: the registry is the daemon's; this is the read side over the mirror.

const H = "h";
const set = (...tasks: ReturnType<typeof taskEntry>[]) =>
	mirrorSet(H, "running", {}, { backgroundTasks: tasks });

beforeEach(() => {
	resetPhaseMirror();
});

describe("backgroundTasks (mirror read side)", () => {
	it("is empty for an unknown harness, with a stable empty list", () => {
		expect(backgroundTasks.live(H)).toEqual([]);
		expect(backgroundTasks.live(H)).toBe(backgroundTasks.live("other"));
		expect(backgroundTasks.workingCount(H)).toBe(0);
	});

	it("counts neither attach-only nor overdue tasks as working, but lists them", () => {
		set(
			taskEntry("a"),
			taskEntry("b", { fromAttach: true }),
			taskEntry("c", { overdue: true }),
			taskEntry("d", { fromAttach: true, overdue: true }),
		);
		expect(backgroundTasks.live(H)).toHaveLength(4);
		expect(backgroundTasks.workingCount(H)).toBe(1);
	});

	it("notifies on a list change only, and stops after unsubscribe", () => {
		let calls = 0;
		const off = backgroundTasks.subscribe(H, () => calls++);
		set(taskEntry("a"));
		expect(calls).toBe(1);
		set(taskEntry("a"));
		expect(calls).toBe(1);
		set(taskEntry("a", { overdue: true }));
		expect(calls).toBe(2);
		off();
		set();
		expect(calls).toBe(2);
	});

	it("takeUnreportedOverdue counts each overdue task once, silently", () => {
		let calls = 0;
		backgroundTasks.subscribe(H, () => calls++);
		set(
			taskEntry("a", { overdue: true }),
			taskEntry("b"),
			taskEntry("c", { fromAttach: true, overdue: true }),
		);
		calls = 0;
		expect(backgroundTasks.takeUnreportedOverdue(H)).toBe(1);
		expect(backgroundTasks.takeUnreportedOverdue(H)).toBe(0);
		set(taskEntry("a", { overdue: true }), taskEntry("b", { overdue: true }));
		calls = 0;
		expect(backgroundTasks.takeUnreportedOverdue(H)).toBe(1);
		expect(calls).toBe(0);
	});

	it("drops the list when the daemon forgets the harness", () => {
		set(taskEntry("a"));
		mirrorForget(H);
		expect(backgroundTasks.live(H)).toEqual([]);
	});
});

describe("deadlineOf", () => {
	const base = { kind: "monitor", persistent: false, timeoutMs: 1000, startedAt: 5 } as const;
	it("is startedAt + timeout for a non-persistent monitor with a timeout", () => {
		expect(deadlineOf(base)).toBe(1005);
	});
	it("is null for everything else", () => {
		expect(deadlineOf({ ...base, kind: "bash" })).toBeNull();
		expect(deadlineOf({ ...base, persistent: true })).toBeNull();
		expect(deadlineOf({ ...base, timeoutMs: null })).toBeNull();
	});
});
