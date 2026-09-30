import { describe, expect, it } from "vitest";
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

	it("never puts an element thread on a line", () => {
		expect(threadsByLine([t({ scope: "element", lineStart: 2 })]).size).toBe(0);
	});
});
