import { describe, expect, it } from "vitest";
import {
	applyResponse,
	contextLeadHarness,
	contextSizeTitle,
	formatContextTokens,
} from "./contextSize.ts";
import type { HarnessKind } from "./types.ts";

const room = (...kinds: HarnessKind[]) => ({
	harnesses: kinds.map((kind, i) => ({ id: `h${i}`, kind })),
});

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

describe("contextSizeTitle", () => {
	it("groups digits", () => {
		expect(contextSizeTitle(142_318, "claude")).toBe(
			"Context: 142,318 tokens — latest turn of claude (input + cache)",
		);
	});
});

describe("applyResponse", () => {
	const leads = new Map([["h1", { roomId: "r1", harnessName: "claude" }]]);
	const row = (tokens: number) => [{ harnessId: "h1", tokens, timestampMs: 0 }];
	const empty = new Map<string, { tokens: number; harnessName: string }>();

	it("applies the newest request", () => {
		const out = applyResponse(empty, new Map([["h1", 2]]), new Map([["h1", 2]]), leads, row(50));
		expect(out.get("r1")).toEqual({ tokens: 50, harnessName: "claude" });
	});

	it("drops a response superseded by a newer request", () => {
		const state = new Map([["r1", { tokens: 90, harnessName: "claude" }]]);
		const out = applyResponse(state, new Map([["h1", 2]]), new Map([["h1", 1]]), leads, row(10));
		expect(out).toBe(state);
	});

	it("does not re-add a room that lost its lead", () => {
		const out = applyResponse(
			empty,
			new Map([["h1", 1]]),
			new Map([["h1", 1]]),
			new Map(),
			row(50),
		);
		expect(out).toBe(empty);
	});

	it("does not apply a pair whose room changed", () => {
		const moved = new Map([["h1", { roomId: "r2", harnessName: "claude" }]]);
		const out = applyResponse(empty, new Map([["h1", 1]]), new Map([["h1", 1]]), moved, row(5));
		expect(out.has("r1")).toBe(false);
		expect(out.get("r2")?.tokens).toBe(5);
	});

	it("clears a room whose harness has no row", () => {
		const state = new Map([["r1", { tokens: 9, harnessName: "claude" }]]);
		const out = applyResponse(state, new Map([["h1", 1]]), new Map([["h1", 1]]), leads, []);
		expect(out.has("r1")).toBe(false);
	});
});

describe("contextLeadHarness", () => {
	it("picks the first claude/opencode harness", () => {
		expect(contextLeadHarness(room("opencode", "claude"))?.id).toBe("h0");
		expect(contextLeadHarness(room("claude", "opencode"))?.id).toBe("h0");
	});
	it("skips a leading shell", () => {
		expect(contextLeadHarness(room("byoh", "claude"))?.id).toBe("h1");
	});
	it("is undefined with no source", () => {
		expect(contextLeadHarness(room("byoh", "files"))).toBeUndefined();
	});
	it("is undefined for an empty room", () => {
		expect(contextLeadHarness(room())).toBeUndefined();
	});
});
