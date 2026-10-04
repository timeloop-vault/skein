import { describe, expect, it } from "vitest";
import {
	changesAnswer,
	LIMITS,
	matchChanges,
	parseShowChangesArgs,
	type SourceSite,
} from "./designChanges.ts";

const url = (p: string) => `http://127.0.0.1:1234/preview/tok/${p}`;
const site = (p: string, line: number, endLine = line, count = 1, onScreen = 1): SourceSite => ({
	fileName: url(p),
	line,
	endLine,
	count,
	onScreen,
});
const file = (path: string, lines: number[], deleted = false) => ({ path, lines, deleted });

describe("matchChanges", () => {
	const sites = [site("a/x.jsx", 10, 13)];

	it("matches the start line, an attribute line and the last line of the span", () => {
		for (const n of [10, 12, 13]) {
			const m = matchChanges(sites, [file("a/x.jsx", [n])]);
			expect(m.mapped).toEqual([
				{ path: "a/x.jsx", line: 10, endLine: 13, changedLines: [n], elements: 1, onScreen: 1 },
			]);
			expect(m.unmapped).toEqual([]);
			expect(m.matchedSites).toEqual([{ fileName: url("a/x.jsx"), line: 10, endLine: 13 }]);
		}
	});

	it("does not match the line after endLine, and before line", () => {
		const m = matchChanges(sites, [file("a/x.jsx", [9, 14])]);
		expect(m.mapped).toEqual([]);
		expect(m.unmapped).toEqual([{ path: "a/x.jsx", lines: [9, 14], reason: "no_rendered_tag" }]);
	});

	it("endLine equal to line matches only that line", () => {
		const m = matchChanges([site("a/x.jsx", 5)], [file("a/x.jsx", [5, 6])]);
		expect(m.mapped[0]?.changedLines).toEqual([5]);
		expect(m.unmapped).toEqual([{ path: "a/x.jsx", lines: [6], reason: "no_rendered_tag" }]);
	});

	it("reports instance counts", () => {
		const m = matchChanges([site("a/x.jsx", 5, 5, 7, 3)], [file("a/x.jsx", [5])]);
		expect(m.mapped[0]).toMatchObject({ elements: 7, onScreen: 3 });
	});

	it("handles two files and several sites in one file", () => {
		const m = matchChanges(
			[site("a/x.jsx", 1, 2), site("a/x.jsx", 20), site("b/y.jsx", 3)],
			[file("a/x.jsx", [2, 20, 30]), file("b/y.jsx", [3])],
		);
		expect(m.mapped.map((x) => [x.path, x.line, x.changedLines])).toEqual([
			["a/x.jsx", 1, [2]],
			["a/x.jsx", 20, [20]],
			["b/y.jsx", 3, [3]],
		]);
		expect(m.unmapped).toEqual([{ path: "a/x.jsx", lines: [30], reason: "no_rendered_tag" }]);
		expect(m.matchedSites).toHaveLength(3);
	});

	it("ignores sites whose fileName cannot be mapped", () => {
		const m = matchChanges(
			[{ fileName: "webpack:///x.jsx", line: 1, endLine: 1, count: 1, onScreen: 1 }],
			[file("x.jsx", [1])],
		);
		expect(m.mapped).toEqual([]);
		expect(m.noSourceInfo).toBe(false);
		expect(m.unmapped).toEqual([{ path: "x.jsx", lines: [1], reason: "file_not_rendered" }]);
	});

	it("keeps sites sharing a start line but not an end line apart", () => {
		const m = matchChanges([site("a/x.jsx", 5, 8), site("a/x.jsx", 5, 5)], [file("a/x.jsx", [7])]);
		expect(m.mapped.map((x) => [x.line, x.endLine])).toEqual([[5, 8]]);
		expect(m.matchedSites).toEqual([{ fileName: url("a/x.jsx"), line: 5, endLine: 8 }]);
		const n = matchChanges([site("a/x.jsx", 5, 8), site("a/x.jsx", 5, 5)], [file("a/x.jsx", [5])]);
		expect(n.matchedSites).toHaveLength(2);
	});

	it("compares paths case-insensitively", () => {
		const m = matchChanges(sites, [file("A/X.jsx", [11])]);
		expect(m.mapped).toHaveLength(1);
		expect(m.unmapped).toEqual([]);
	});

	it("propagates a capped site list", () => {
		const m = matchChanges(sites, [file("zzz.jsx", [1])], true);
		expect(m.sitesCapped).toBe(true);
		expect(m.unmapped[0]?.reason).toBe("file_not_rendered");
		expect(changesAnswer(m, { highlighted: 0 }, false).sitesCapped).toBe(true);
		expect("sitesCapped" in changesAnswer(matchChanges(sites, []), { highlighted: 0 }, false)).toBe(
			false,
		);
	});

	it("reports deleted files", () => {
		const m = matchChanges(sites, [file("a/x.jsx", [], true)]);
		expect(m.unmapped).toEqual([{ path: "a/x.jsx", reason: "deleted" }]);
	});

	it("reports files with no rendered site", () => {
		const m = matchChanges(sites, [file("styles.css", [4, 2, 4])]);
		expect(m.unmapped).toEqual([
			{ path: "styles.css", lines: [2, 4], reason: "file_not_rendered" },
		]);
	});

	it("flags noSourceInfo when the page reports no sites", () => {
		const m = matchChanges([], [file("a/x.jsx", [1])]);
		expect(m.noSourceInfo).toBe(true);
		expect(m.unmapped[0]?.reason).toBe("file_not_rendered");
	});

	it("normalises Windows-style separators in changed paths", () => {
		const m = matchChanges(sites, [file("a\\x.jsx", [11])]);
		expect(m.mapped).toHaveLength(1);
		expect(m.unmapped).toEqual([]);
	});
});

describe("changesAnswer", () => {
	const match = matchChanges([site("a.jsx", 1)], [file("a.jsx", [1])]);

	it("carries limits, passes capped and truncated through", () => {
		const r = changesAnswer(match, { highlighted: 200, capped: true }, true);
		expect(r).toMatchObject({ highlighted: 200, capped: true, truncated: true, limits: LIMITS });
		expect(r.noSourceInfo).toBeUndefined();
	});

	it("omits optional flags when unset", () => {
		const r = changesAnswer(match, { highlighted: 1 }, false);
		expect("capped" in r).toBe(false);
		expect("truncated" in r).toBe(false);
	});
});

describe("parseShowChangesArgs", () => {
	const base = { roomId: "r", harnessId: "h", files: [{ path: "a.jsx", lines: [1, 2] }] };

	it("accepts a well-formed request", () => {
		const r = parseShowChangesArgs({ ...base, truncated: true, reveal: true });
		expect(r).toEqual({
			ok: true,
			value: {
				roomId: "r",
				harnessId: "h",
				files: [{ path: "a.jsx", lines: [1, 2], deleted: false }],
				truncated: true,
				reveal: true,
			},
		});
	});

	it("rejects bad shapes", () => {
		for (const bad of [
			null,
			{ ...base, roomId: "" },
			{ ...base, harnessId: 1 },
			{ ...base, files: "x" },
			{ ...base, files: [null] },
			{ ...base, files: [{ path: "", lines: [] }] },
			{ ...base, files: [{ path: "a", lines: ["1"] }] },
			{ ...base, files: [{ path: "a", lines: [1.5] }] },
			{ ...base, files: [{ path: "a", lines: [], deleted: "yes" }] },
			{ ...base, reveal: "yes" },
		]) {
			const r = parseShowChangesArgs(bad);
			expect(r.ok).toBe(false);
			if (!r.ok) expect(r.error.startsWith("bad_arguments")).toBe(true);
		}
	});

	it("allows an empty files list and a deleted file", () => {
		expect(parseShowChangesArgs({ ...base, files: [] }).ok).toBe(true);
		const r = parseShowChangesArgs({ ...base, files: [{ path: "a", lines: [], deleted: true }] });
		expect(r.ok && r.value.files[0]?.deleted).toBe(true);
	});
});
