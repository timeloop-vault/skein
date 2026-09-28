import { describe, expect, it } from "vitest";
import { chooseReviewBody, isSvgPath, modesAvailable, sidesForChange } from "./imageDiffModel.ts";

describe("isSvgPath", () => {
	it("is true only for .svg", () => {
		expect(isSvgPath("icon.svg")).toBe(true);
		expect(isSvgPath("photo.png")).toBe(false);
		expect(isSvgPath("notes.txt")).toBe(false);
	});
});

describe("chooseReviewBody", () => {
	const base = { hasFile: true, blocked: undefined, hunksLength: 3, svgMode: "image" as const };

	it("shows nothing selected when there is no file", () => {
		expect(chooseReviewBody({ ...base, hasFile: false, path: "" })).toEqual({ kind: "no-file" });
	});

	it("renders a raster image as image, blocked or not", () => {
		expect(chooseReviewBody({ ...base, path: "shot.png", blocked: "binary" })).toEqual({
			kind: "image",
		});
		expect(chooseReviewBody({ ...base, path: "shot.png", blocked: undefined })).toEqual({
			kind: "image",
		});
	});

	it("renders an svg as image by default", () => {
		expect(chooseReviewBody({ ...base, path: "icon.svg" })).toEqual({ kind: "image" });
	});

	it("falls through to the diff logic for an svg in text mode", () => {
		expect(chooseReviewBody({ ...base, path: "icon.svg", svgMode: "text" })).toEqual({
			kind: "diff",
		});
		expect(
			chooseReviewBody({ ...base, path: "icon.svg", svgMode: "text", hunksLength: 0 }),
		).toEqual({ kind: "no-diff" });
		expect(
			chooseReviewBody({ ...base, path: "icon.svg", svgMode: "text", blocked: "symlink" }),
		).toEqual({ kind: "blocked", blocked: "symlink" });
	});

	it("renders a non-image blocked file as blocked", () => {
		expect(chooseReviewBody({ ...base, path: "archive.zip", blocked: "binary" })).toEqual({
			kind: "blocked",
			blocked: "binary",
		});
	});

	it("renders a normal text file with no hunks as no-diff, otherwise diff", () => {
		expect(chooseReviewBody({ ...base, path: "a.ts", hunksLength: 0 })).toEqual({
			kind: "no-diff",
		});
		expect(chooseReviewBody({ ...base, path: "a.ts" })).toEqual({ kind: "diff" });
	});
});

describe("sidesForChange", () => {
	it("added/untracked have only a new side", () => {
		expect(sidesForChange("added")).toEqual({ old: false, new: true });
		expect(sidesForChange("untracked")).toEqual({ old: false, new: true });
	});

	it("deleted has only an old side", () => {
		expect(sidesForChange("deleted")).toEqual({ old: true, new: false });
	});

	it("modified, renamed and anything else have both sides", () => {
		expect(sidesForChange("modified")).toEqual({ old: true, new: true });
		expect(sidesForChange("renamed")).toEqual({ old: true, new: true });
		expect(sidesForChange("conflicted")).toEqual({ old: true, new: true });
	});
});

describe("modesAvailable", () => {
	it("enables swap and onion only when both sides are ready", () => {
		expect(modesAvailable("ready", "ready")).toEqual({ swap: true, onion: true });
	});

	it.each([
		["loading", "ready"],
		["ready", "absent"],
		["error", "ready"],
		["absent", "absent"],
	] as const)("disables both when a side is %s/%s", (old, next) => {
		expect(modesAvailable(old, next)).toEqual({ swap: false, onion: false });
	});
});
