import { describe, expect, it } from "vitest";
import { applyResponse, contextLabel, formatContextTokens, maxContext } from "./contextSize.ts";

describe("formatContextTokens", () => {
	it.each([
		[0, "0"],
		[999, "999"],
		[1000, "1k"],
		[1049, "1k"],
		[1050, "1.1k"],
		[1234, "1.2k"],
		[9999, "10k"],
		[10_000, "10k"],
		[142_318, "142k"],
		[999_499, "999k"],
		[999_500, "1M"],
		[1_234_567, "1.2M"],
	])("%i -> %s", (n, out) => {
		expect(formatContextTokens(n)).toBe(out);
	});
});

describe("contextLabel", () => {
	it("formats", () => {
		expect(contextLabel(142_318)).toBe("context 142k");
		expect(contextLabel(999)).toBe("context 999");
	});
});

describe("maxContext", () => {
	const hs = [
		{ id: "a", name: "A" },
		{ id: "b", name: "B" },
		{ id: "c", name: "C" },
	];
	const from = (m: Record<string, number>) => (id: string) => m[id];
	it("is undefined with no values", () => {
		expect(maxContext(hs, from({}))).toBeUndefined();
		expect(maxContext([], from({ a: 1 }))).toBeUndefined();
	});
	it("returns a single value", () => {
		expect(maxContext(hs, from({ b: 7 }))).toEqual({ harnessId: "b", harnessName: "B", tokens: 7 });
	});
	it("returns the largest", () => {
		expect(maxContext(hs, from({ a: 1, b: 9, c: 5 }))?.harnessId).toBe("b");
	});
	it("ties go to the first in order", () => {
		expect(maxContext(hs, from({ a: 4, c: 4 }))?.harnessId).toBe("a");
	});
	it("skips undefined values", () => {
		expect(maxContext(hs, from({ a: 3, c: 2 }))?.harnessId).toBe("a");
	});
});

describe("applyResponse", () => {
	const tracked = new Set(["h1"]);
	const row = (tokens: number) => [{ harnessId: "h1", tokens, timestampMs: 0 }];
	const empty = new Map<string, number>();
	const one = new Map([["h1", 1]]);

	it("applies the newest request", () => {
		const out = applyResponse(empty, new Map([["h1", 2]]), new Map([["h1", 2]]), tracked, row(50));
		expect(out.get("h1")).toBe(50);
	});

	it("drops a response superseded by a newer request", () => {
		const state = new Map([["h1", 90]]);
		const out = applyResponse(state, new Map([["h1", 2]]), one, tracked, row(10));
		expect(out).toBe(state);
	});

	it("does not re-add an untracked harness", () => {
		const out = applyResponse(empty, one, one, new Set(), row(50));
		expect(out).toBe(empty);
	});

	it("returns the same state when the value is unchanged", () => {
		const state = new Map([["h1", 50]]);
		expect(applyResponse(state, one, one, tracked, row(50))).toBe(state);
	});

	it("clears a harness that has no row", () => {
		const out = applyResponse(new Map([["h1", 9]]), one, one, tracked, []);
		expect(out.has("h1")).toBe(false);
	});
});
