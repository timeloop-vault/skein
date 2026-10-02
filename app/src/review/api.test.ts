import { describe, expect, it } from "vitest";
import type { ReviewHunk } from "../liveContext/review.ts";
import { type ReviewThread, threadsByLine, unplacedThreads } from "./api.ts";

const t = (over: Partial<ReviewThread>): ReviewThread => ({
	id: "t",
	scope: "line",
	anchorLines: [],
	placement: "unmoved",
	outdated: false,
	createdMs: 0,
	updatedMs: 0,
	comments: [],
	...over,
});

const hunk = (nums: number[]): ReviewHunk =>
	({
		header: "@@",
		oldStart: 1,
		newStart: nums[0] ?? 1,
		lines: nums.map((n) => ({ kind: "context", content: "x", oldLineno: n, newLineno: n })),
	}) as unknown as ReviewHunk;

describe("unplacedThreads", () => {
	it("includes file, element and unplaced line threads, not placed ones", () => {
		const threads = [
			t({ id: "file", scope: "file" }),
			t({ id: "el", scope: "element", placement: "element" }),
			t({ id: "lost", scope: "line" }),
			t({ id: "placed", scope: "line", lineStart: 3 }),
			t({ id: "rev", scope: "review" }),
		];
		expect(unplacedThreads(threads).map((x) => x.id)).toEqual(["file", "el", "lost"]);
	});

	it("never puts a plain element thread on a line", () => {
		const el = t({ scope: "element", lineStart: 2 });
		expect(threadsByLine([el], [hunk([2])]).size).toBe(0);
		expect(unplacedThreads([el], [hunk([2])])).toHaveLength(1);
	});
});

describe("viaSource element threads (#467)", () => {
	const via = (over: Partial<ReviewThread> = {}) =>
		t({ id: "v", scope: "element", viaSource: true, lineStart: 5, ...over });

	it("is placed inline when its line is rendered in a hunk", () => {
		const th = via();
		expect(threadsByLine([th], [hunk([4, 5, 6])]).get("new:5")).toEqual([th]);
		expect(unplacedThreads([th], [hunk([4, 5, 6])])).toEqual([]);
	});

	it("goes above the diff when the file has no hunks", () => {
		const th = via();
		expect(threadsByLine([th], []).size).toBe(0);
		expect(unplacedThreads([th], [])).toEqual([th]);
	});

	it("goes above the diff when its line is not rendered", () => {
		expect(unplacedThreads([via()], [hunk([10, 11])])).toHaveLength(1);
	});

	it("goes above the diff when outdated", () => {
		const th = via({ outdated: true, placement: "outdated" });
		delete th.lineStart;
		expect(threadsByLine([th], [hunk([5])]).size).toBe(0);
		expect(unplacedThreads([th], [hunk([5])])).toEqual([th]);
	});
});
