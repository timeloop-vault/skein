import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));

import { backgroundTasks } from "./backgroundTasks.ts";
import { harnessActivity } from "./harnessActivity.ts";
import {
	applyHermesWork,
	clearHermesWork,
	type HermesWorkEvent,
	RECENTLY_ENDED_MS,
} from "./hermesWork.ts";
import { subagents } from "./subagents.ts";

const H = "hw";
const apply = (e: HermesWorkEvent) => applyHermesWork(H, e);

describe("Hermes work stream mapping (#628)", () => {
	it("an initial subagent is listed but not working; a live one is", () => {
		apply({ kind: "subagentStart", agentId: "old", description: null, initial: true });
		expect(subagents.live(H)).toHaveLength(1);
		expect(subagents.workingCount(H)).toBe(0);
		apply({ kind: "subagentStart", agentId: "new", description: "g", initial: false });
		expect(subagents.workingCount(H)).toBe(1);
		apply({ kind: "subagentEnd", agentId: "old" });
		apply({ kind: "subagentEnd", agentId: "new" });
		expect(subagents.live(H)).toHaveLength(0);
	});

	it("an initial background task is not working; a live one is, as a bash task", () => {
		apply({ kind: "backgroundStart", taskId: "a", command: "x", initial: true });
		apply({ kind: "backgroundStart", taskId: "b", command: "y", initial: false });
		expect(backgroundTasks.workingCount(H)).toBe(1);
		expect(backgroundTasks.live(H).map((t) => t.kind)).toEqual(["bash", "bash"]);
		apply({ kind: "backgroundEnd", taskId: "a" });
		apply({ kind: "backgroundEnd", taskId: "b" });
		expect(backgroundTasks.live(H)).toHaveLength(0);
	});

	describe("recently ended echo", () => {
		const live = (id: string) =>
			apply({ kind: "subagentStart", agentId: id, description: null, initial: false });
		afterEach(() => {
			vi.useRealTimers();
			clearHermesWork(H);
			harnessActivity.forget(H);
		});

		it("ignores a live start within the window, counts one after it", () => {
			vi.useFakeTimers();
			harnessActivity.spawned(H);
			live("e");
			apply({ kind: "subagentEnd", agentId: "e" });
			const reclaim = vi.spyOn(harnessActivity, "reclaimDelegationFromAdapter");
			vi.advanceTimersByTime(RECENTLY_ENDED_MS - 1);
			live("e");
			expect(subagents.workingCount(H)).toBe(0);
			expect(reclaim).not.toHaveBeenCalled();
			vi.advanceTimersByTime(2);
			live("e");
			expect(subagents.workingCount(H)).toBe(1);
			expect(reclaim).toHaveBeenCalledTimes(1);
			reclaim.mockRestore();
		});

		it("an initial start is unaffected and clearHermesWork forgets the tombstone", () => {
			vi.useFakeTimers();
			live("f");
			apply({ kind: "subagentEnd", agentId: "f" });
			apply({ kind: "subagentStart", agentId: "f", description: null, initial: true });
			expect(subagents.live(H)).toHaveLength(1);
			clearHermesWork(H);
			live("f");
			expect(subagents.workingCount(H)).toBe(1);
		});
	});

	it("double-reporting (hook and disk) is harmless", () => {
		harnessActivity.spawned(H);
		apply({ kind: "subagentStart", agentId: "c", description: null, initial: false });
		apply({ kind: "subagentStart", agentId: "c", description: null, initial: false });
		expect(subagents.workingCount(H)).toBe(1);
		expect(harnessActivity.get(H)?.delegatedCount).toBe(1);
		apply({ kind: "subagentEnd", agentId: "c" });
		apply({ kind: "subagentEnd", agentId: "c" });
		apply({ kind: "subagentEnd", agentId: "never" });
		apply({ kind: "backgroundEnd", taskId: "never" });
		expect(subagents.workingCount(H)).toBe(0);
		harnessActivity.forget(H);
	});
});
