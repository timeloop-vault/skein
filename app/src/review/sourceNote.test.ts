import { describe, expect, it } from "vitest";
import type { ReviewThread } from "./api.ts";
import { sourceNoteText } from "./sourceNote.ts";

const th = (over: Partial<ReviewThread> = {}): ReviewThread => ({
	id: "t",
	scope: "element",
	filePath: "design/index.html",
	viaSource: true,
	lineStart: 7,
	anchorLines: [],
	placement: "unmoved",
	outdated: false,
	createdMs: 0,
	updatedMs: 0,
	comments: [],
	...over,
});

describe("sourceNoteText", () => {
	it("says shown here when inline", () => {
		expect(sourceNoteText(th(), true)).toBe(
			"Design comment on index.html — shown here at its source line",
		);
	});
	it("says the diff doesn't show the line when above the diff", () => {
		expect(sourceNoteText(th(), false)).toBe(
			"Design comment on index.html — at line 7, which this diff doesn't show",
		);
	});
});
