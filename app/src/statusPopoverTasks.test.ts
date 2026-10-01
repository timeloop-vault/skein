import { describe, expect, it } from "vitest";
import type { BackgroundTaskEntry } from "./backgroundTasks.ts";
import { moreTasksText, tasksSegmentText } from "./statusPopoverTasks.ts";

const task = (over: Partial<BackgroundTaskEntry>): BackgroundTaskEntry =>
	({
		id: "t",
		kind: "bash",
		description: "build",
		startedAt: 0,
		fromAttach: false,
		overdue: false,
		...over,
	}) as BackgroundTaskEntry;

describe("tasksSegmentText", () => {
	it("is null with no tasks", () => {
		expect(tasksSegmentText([], 0)).toBeNull();
	});
	it("prints one task", () => {
		expect(tasksSegmentText([task({})], 5000)).toBe('bash "build" · 5s');
	});
	it("joins two with a semicolon", () => {
		expect(tasksSegmentText([task({}), task({ description: "test" })], 5000)).toBe(
			'bash "build" · 5s; bash "test" · 5s',
		);
	});
	it("caps at two and counts the rest", () => {
		const t = [task({}), task({}), task({}), task({})];
		expect(tasksSegmentText(t, 5000)).toBe('bash "build" · 5s; bash "build" · 5s +2 more');
	});
});

describe("moreTasksText", () => {
	it("mirrors the subagent wording", () => {
		expect(moreTasksText(3)).toBe("↳ + 3 more");
	});
});
