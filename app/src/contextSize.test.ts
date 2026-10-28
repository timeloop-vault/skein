import { describe, expect, it } from "vitest";
import {
	anyOver,
	applyResponse,
	contextLabel,
	formatContextTokens,
	isOverThreshold,
	parseTokenCount,
	thresholdDisplay,
	thresholdTextChanged,
} from "./contextSize.ts";

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

describe("isOverThreshold", () => {
	it.each([
		[200_000, undefined, false],
		[200_000, null, false],
		[200_000, 0, false],
		[200_000, -5, false],
		[undefined, 100_000, false],
		[100_000, 100_000, false],
		[100_001, 100_000, true],
		[50_000, 100_000, false],
	])("%s vs %s -> %s", (tokens, threshold, out) => {
		expect(isOverThreshold(tokens, threshold)).toBe(out);
	});
});

describe("anyOver", () => {
	const sizes = new Map([
		["a", 10],
		["b", 500],
	]);
	const get = (id: string) => sizes.get(id);
	it("is true when one id is over", () => expect(anyOver(["a", "b"], get, 100)).toBe(true));
	it("is false when none is", () => expect(anyOver(["a", "x"], get, 100)).toBe(false));
	it("is false when off or empty", () => {
		expect(anyOver(["b"], get, null)).toBe(false);
		expect(anyOver([], get, 100)).toBe(false);
	});
});

describe("parseTokenCount", () => {
	it.each([
		["150000", 150_000],
		["150k", 150_000],
		["150K", 150_000],
		["1.2M", 1_200_000],
		["1.2m", 1_200_000],
		[" 150 k ", 150_000],
		["150,000", 150_000],
		["1_200_000", 1_200_000],
		["0.5k", 500],
		[".5m", 500_000],
	])("parses %j as %i", (text, want) => {
		expect(parseTokenCount(text)).toBe(want);
	});

	it("treats empty as off", () => {
		expect(parseTokenCount("")).toBeNull();
		expect(parseTokenCount("  ")).toBeNull();
	});

	it.each([
		"abc",
		"0",
		"0k",
		"-5",
		"1.2.3k",
		"12kk",
		"k",
		"1e5",
		"10x",
		"1,5M",
		"1 5",
		"1,50",
		"1,2345",
		"0.4",
	])("rejects %j", (text) => {
		expect(parseTokenCount(text)).toBe("invalid");
	});
});

describe("thresholdDisplay / thresholdTextChanged", () => {
	it("shows the exact count when compacting would round", () => {
		expect(thresholdDisplay(150_000)).toBe("150k");
		expect(thresholdDisplay(150_500)).toBe("150500");
		expect(thresholdDisplay(1234)).toBe("1234");
		expect(thresholdDisplay(null)).toBe("");
	});

	it("does not treat an untouched field as a change", () => {
		for (const v of [150_000, 150_500, 1234, null, undefined]) {
			expect(thresholdTextChanged(thresholdDisplay(v), v)).toBe(false);
		}
		expect(thresholdTextChanged("160k", 150_000)).toBe(true);
	});
});
