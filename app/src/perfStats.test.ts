import { describe, expect, it } from "vitest";
import {
	formatPerfSummary,
	percentile,
	SampleBuffer,
	SpanAccumulator,
	summarise,
} from "./perfStats.ts";

describe("percentile / summarise", () => {
	it("empty is all zero", () => {
		expect(summarise([])).toEqual({ count: 0, p50: 0, p95: 0, max: 0 });
		expect(percentile([], 50)).toBe(0);
	});
	it("one sample", () => {
		expect(summarise([7])).toEqual({ count: 1, p50: 7, p95: 7, max: 7 });
	});
	it("odd count, nearest rank", () => {
		expect(summarise([5, 1, 3])).toEqual({ count: 3, p50: 3, p95: 5, max: 5 });
	});
	it("even count, nearest rank takes the lower middle", () => {
		const s = summarise([4, 1, 3, 2]);
		expect(s.p50).toBe(2);
		expect(s.p95).toBe(4);
		expect(s.max).toBe(4);
	});
	it("p95 of 1..100 is 95", () => {
		const a = Array.from({ length: 100 }, (_, i) => i + 1);
		expect(summarise(a).p95).toBe(95);
	});
});

describe("thresholds", () => {
	it("counts strictly over", () => {
		const b = new SampleBuffer();
		for (const v of [10, 50, 51, 200, 201]) b.add(v);
		expect(b.summary().over50).toBe(3);
		expect(b.summary().over200).toBe(1);
	});
});

describe("SampleBuffer", () => {
	it("keeps exact count, max and thresholds past the cap", () => {
		const b = new SampleBuffer(8, () => 0.5);
		for (let i = 0; i < 1000; i++) b.add(i === 500 ? 300 : 1);
		const s = b.summary();
		expect(s.count).toBe(1000);
		expect(s.max).toBe(300);
		expect(s.over50).toBe(1);
		expect(s.over200).toBe(1);
	});
	it("never holds more than cap samples", () => {
		const b = new SampleBuffer(4, () => 0);
		for (let i = 0; i < 100; i++) b.add(i);
		// biome-ignore lint/suspicious/noExplicitAny: peek at private state
		expect(((b as any).items as number[]).length).toBe(4);
		expect(b.count).toBe(100);
	});
	it("reset clears", () => {
		const b = new SampleBuffer();
		b.add(60);
		b.reset();
		expect(b.summary()).toEqual({ count: 0, p50: 0, p95: 0, max: 0, over50: 0, over200: 0 });
	});
});

describe("SpanAccumulator", () => {
	it("accumulates per area", () => {
		const a = new SpanAccumulator<"x" | "y">();
		a.record("x", 2);
		a.record("x", 6);
		a.record("y", 1, "wall");
		expect(a.snapshot()).toEqual([
			["x", { count: 2, totalMs: 8, maxMs: 6, kind: "main" }],
			["y", { count: 1, totalMs: 1, maxMs: 1, kind: "wall" }],
		]);
		a.reset();
		expect(a.snapshot()).toEqual([]);
	});
});

describe("formatPerfSummary", () => {
	it("builds one line, omits empty areas, n/a longtasks", () => {
		const line = formatPerfSummary({
			reason: "interval",
			windowSec: 300,
			drift: { count: 1180, p50: 1, p95: 6, max: 84, over50: 3, over200: 0 },
			longtasks: 4,
			rooms: 7,
			harnesses: 15,
			spans: [
				["liveContext.apply", { count: 812, totalMs: 410, maxMs: 22, kind: "main" }],
				["rooms.save", { count: 0, totalMs: 0, maxMs: 0, kind: "wall" }],
				["branchWatch.tick", { count: 31, totalMs: 95, maxMs: 12, kind: "wall" }],
			],
		});
		expect(line).toBe(
			"perf summary reason=interval window=300s drift p50=1ms p95=6ms max=84ms samples=1180 stalls>50ms=3 stalls>200ms=0 longtasks=4 rooms=7 harnesses=15 | liveContext.apply n=812 total=410ms max=22ms | branchWatch.tick[wall] n=31 total=95ms max=12ms",
		);
		expect(line).not.toContain("\n");
		expect(
			formatPerfSummary({
				reason: "shutdown",
				windowSec: 5,
				drift: { count: 0, p50: 0, p95: 0, max: 0, over50: 0, over200: 0 },
				longtasks: null,
				rooms: 0,
				harnesses: 0,
				spans: [],
			}),
		).toContain("longtasks=n/a");
	});
});
