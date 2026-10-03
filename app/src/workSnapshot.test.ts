import { afterEach, describe, expect, it } from "vitest";
import { backgroundTasks } from "./backgroundTasks.ts";
import { workSnapshot } from "./deferral.ts";
import { subagents } from "./subagents.ts";

const IDS = ["h1", "h2", "h3"];
const agent = (agentId: string) => ({ agentId, agentType: null, description: null });
const task = (taskId: string) => ({
	taskId,
	kind: "bash" as const,
	description: null,
	command: null,
	timeoutMs: null,
	persistent: false,
	agentId: null,
});

afterEach(() => {
	for (const id of IDS) {
		subagents.forget(id);
		backgroundTasks.forget(id);
	}
});

describe("workSnapshot (#448)", () => {
	it("reports both counts per harness and 0/0 for an idle one", () => {
		subagents.record("h1", agent("a1"), false);
		subagents.record("h1", agent("a2"), false);
		backgroundTasks.record("h1", task("t1"), false);
		backgroundTasks.record("h2", task("t2"), false);
		expect(workSnapshot(IDS)).toEqual({
			h1: { subagents: 2, backgroundTasks: 1 },
			h2: { subagents: 0, backgroundTasks: 1 },
			h3: { subagents: 0, backgroundTasks: 0 },
		});
	});

	it("excludes attach-seeded subagents and tasks", () => {
		subagents.record("h1", agent("a1"), true);
		backgroundTasks.record("h1", task("t1"), true);
		expect(workSnapshot(["h1"])).toEqual({ h1: { subagents: 0, backgroundTasks: 0 } });
	});

	it("excludes overdue tasks", () => {
		backgroundTasks.record("h1", task("t1"), false);
		backgroundTasks.record("h1", task("t2"), false);
		backgroundTasks.markOverdue("h1", ["t1"]);
		expect(workSnapshot(["h1"])).toEqual({ h1: { subagents: 0, backgroundTasks: 1 } });
	});
});
