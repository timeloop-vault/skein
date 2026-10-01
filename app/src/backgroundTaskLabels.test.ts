import { describe, expect, it } from "vitest";
import {
	formatElapsed,
	taskKindLabel,
	taskLine,
	taskTitle,
	workLabel,
} from "./backgroundTaskLabels.ts";
import type { BackgroundTaskEntry } from "./backgroundTasks.ts";

// #447 — pure wording for background tasks.

const entry = (over: Partial<BackgroundTaskEntry> = {}): BackgroundTaskEntry => ({
	taskId: "t1",
	kind: "bash",
	description: "build the app",
	command: null,
	timeoutMs: null,
	persistent: false,
	agentId: null,
	startedAt: 0,
	fromAttach: false,
	overdue: false,
	overdueReported: false,
	...over,
});

describe("workLabel", () => {
	it.each([
		[0, 0, null],
		[1, 0, "delegating · 1 agent"],
		[2, 0, "delegating · 2 agents"],
		[0, 1, "background · 1 task"],
		[0, 3, "background · 3 tasks"],
		[2, 1, "delegating · 2 agents, 1 task"],
		[1, 3, "delegating · 1 agent, 3 tasks"],
	])("%i agents, %i tasks", (agents, tasks, want) => {
		expect(workLabel(agents, tasks)).toBe(want);
	});
});

describe("taskKindLabel", () => {
	it.each(["bash", "powershell", "monitor"] as const)("%s", (k) => {
		expect(taskKindLabel(k)).toBe(k);
	});
});

describe("taskTitle", () => {
	it("prefers the description", () => {
		expect(taskTitle({ description: " build ", command: "make" })).toBe("build");
	});
	it("falls back to the first line of the command", () => {
		expect(taskTitle({ description: "  ", command: "  npm test\nnpm run lint" })).toBe("npm test");
	});
	it("truncates a long command with an ellipsis", () => {
		const t = taskTitle({ description: null, command: "x".repeat(100) });
		expect(t).toHaveLength(60);
		expect(t.endsWith("…")).toBe(true);
	});
	it("is 'task' with nothing to go on", () => {
		expect(taskTitle({ description: null, command: null })).toBe("task");
		expect(taskTitle({ description: null, command: "  \n" })).toBe("task");
	});
});

describe("formatElapsed", () => {
	it.each([
		[-5, "0s"],
		[0, "0s"],
		[45_000, "45s"],
		[60_000, "1m 0s"],
		[192_000, "3m 12s"],
		[3_840_000, "1h 4m"],
	])("%i ms", (ms, want) => {
		expect(formatElapsed(ms)).toBe(want);
	});
});

describe("taskLine", () => {
	it("prints kind, title and elapsed", () => {
		expect(taskLine(entry({ startedAt: 1_000 }), 193_000)).toBe('bash "build the app" · 3m 12s');
	});
	it("shows time left for a Monitor with a deadline, and due once past", () => {
		const m = entry({ kind: "monitor", timeoutMs: 300_000, startedAt: 0 });
		expect(taskLine(m, 60_000)).toBe('monitor "build the app" · 1m 0s · 4m 0s left');
		expect(taskLine(m, 300_000)).toBe('monitor "build the app" · 5m 0s · due');
	});
	it("marks pre-restart and no-longer-waited-on tasks", () => {
		expect(taskLine(entry({ fromAttach: true }), 5_000)).toBe(
			'bash "build the app" · 5s (pre-restart)',
		);
		expect(taskLine(entry({ overdue: true }), 5_000)).toBe(
			'bash "build the app" · 5s (no longer waited on)',
		);
	});
});
