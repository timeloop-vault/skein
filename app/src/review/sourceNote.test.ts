import { describe, expect, it } from "vitest";
import type { ReviewThread } from "./api.ts";
import { sourceHeaderText, sourceNoteText } from "./sourceNote.ts";

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
	it("adds nothing when placed inline", () => {
		expect(sourceNoteText(th(), true)).toBeNull();
	});
	it("says the diff doesn't show the line when above the diff", () => {
		expect(sourceNoteText(th(), false)).toBe("at line 7, which this diff doesn't show");
	});
	it("says the line can't be confirmed when it is outdated", () => {
		expect(sourceNoteText(th({ outdated: true, anchorLines: ["x"] }), true)).toBe(
			"Recorded at its source line; that line can't be confirmed in this file any more",
		);
	});
});

describe("sourceHeaderText", () => {
	it("names the entry basename without an element", () => {
		expect(sourceHeaderText(th())).toBe("◆ design · index.html");
	});
	it("adds the tag and the id or selector, clipped", () => {
		const element = {
			anchor: { tag: "button", selector: "#a".repeat(40), odId: "hero" },
		} as unknown as NonNullable<ReviewThread["element"]>;
		expect(sourceHeaderText(th({ element }))).toBe("◆ design · index.html · button hero");
		const long = {
			anchor: { tag: "p", selector: "x".repeat(60) },
		} as unknown as NonNullable<ReviewThread["element"]>;
		expect(sourceHeaderText(th({ element: long }))).toBe(
			`◆ design · index.html · p ${"x".repeat(48)}…`,
		);
	});
});
