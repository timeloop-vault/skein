import { describe, expect, it } from "vitest";
import {
	CUSTOM_MAX,
	CUSTOM_MIN,
	type DesignDevice,
	deviceLabel,
	deviceParams,
	fitScale,
	frameSize,
	normalizeDevice,
	parseDim,
	portraitSize,
} from "./designDevice.ts";

describe("normalizeDevice", () => {
	const undef: [string, unknown][] = [
		["undefined", undefined],
		["null", null],
		["string", "iphone-14"],
		["number", 3],
		["array", []],
		["empty object", {}],
		["none", { preset: "none" }],
		["non-string preset", { preset: 4 }],
		["unknown preset", { preset: "nokia-3310" }],
		["custom without size", { preset: "custom" }],
		["custom missing height", { preset: "custom", width: 500 }],
		["custom NaN", { preset: "custom", width: Number.NaN, height: 500 }],
		["custom string size", { preset: "custom", width: "500", height: 500 }],
		["custom infinite", { preset: "custom", width: Number.POSITIVE_INFINITY, height: 5 }],
	];
	it.each(undef)("%s -> undefined", (_n, raw) => {
		expect(normalizeDevice(raw)).toBeUndefined();
	});
	it("keeps a DPR on none, and none stays unsized", () => {
		const d = normalizeDevice({ preset: "none", dpr: 2, landscape: true });
		expect(d).toEqual({ preset: "none", dpr: 2 });
		expect(frameSize(d)).toBeNull();
		expect(deviceParams(d)).toEqual([["dpr", "2"]]);
		expect(normalizeDevice({ preset: "none", dpr: 99 })).toBeUndefined();
	});
	it("parseDim accepts in-range integers only", () => {
		expect(parseDim("390")).toBe(390);
		expect(parseDim(" 400 ")).toBe(400);
		for (const bad of ["", "abc", "12.5", String(CUSTOM_MIN - 1), String(CUSTOM_MAX + 1)]) {
			expect(parseDim(bad)).toBeUndefined();
		}
	});
	it("keeps a plain preset", () => {
		expect(normalizeDevice({ preset: "iphone-14" })).toEqual({ preset: "iphone-14" });
	});
	it("drops unknown keys and ignores width/height on a preset", () => {
		expect(
			normalizeDevice({ preset: "android", width: 5, touch: true, landscape: true, dpr: 2 }),
		).toEqual({ preset: "android", landscape: true, dpr: 2 });
	});
	it("keeps landscape only when true", () => {
		expect(normalizeDevice({ preset: "android", landscape: "yes" })).toEqual({
			preset: "android",
		});
		expect(normalizeDevice({ preset: "android", landscape: false })).toEqual({
			preset: "android",
		});
	});
	it("rounds and clamps custom sizes", () => {
		expect(normalizeDevice({ preset: "custom", width: 500.6, height: 700.2 })).toEqual({
			preset: "custom",
			width: 501,
			height: 700,
		});
		expect(normalizeDevice({ preset: "custom", width: 1, height: 99999 })).toEqual({
			preset: "custom",
			width: CUSTOM_MIN,
			height: CUSTOM_MAX,
		});
	});
	it.each([
		[0.5, 0.5],
		[4, 4],
		[2, 2],
		[1.5, 1.5],
		[0.4, undefined],
		[4.1, undefined],
		[Number.NaN, undefined],
		["2", undefined],
	])("dpr %s -> %s", (dpr, want) => {
		expect(normalizeDevice({ preset: "iphone-14", dpr })?.dpr).toBe(want);
	});
});

describe("frameSize", () => {
	it("is null for none/undefined", () => {
		expect(frameSize(undefined)).toBeNull();
		expect(frameSize({ preset: "none" })).toBeNull();
	});
	it("uses preset dims, swapped in landscape", () => {
		expect(frameSize({ preset: "iphone-14" })).toEqual({ width: 390, height: 844 });
		expect(frameSize({ preset: "iphone-14", landscape: true })).toEqual({
			width: 844,
			height: 390,
		});
	});
	it("uses custom dims", () => {
		const d: DesignDevice = { preset: "custom", width: 500, height: 700 };
		expect(frameSize(d)).toEqual({ width: 500, height: 700 });
		expect(frameSize({ ...d, landscape: true })).toEqual({ width: 700, height: 500 });
	});
});

describe("fitScale", () => {
	const frame = { width: 400, height: 800 };
	it("is 1 when it fits", () => {
		expect(fitScale(frame, { width: 1000, height: 1000 })).toBe(1);
	});
	it("shrinks by width", () => {
		expect(fitScale(frame, { width: 200, height: 2000 })).toBe(0.5);
	});
	it("shrinks by height", () => {
		expect(fitScale(frame, { width: 2000, height: 400 })).toBe(0.5);
	});
	it("never exceeds 1", () => {
		expect(fitScale({ width: 10, height: 10 }, { width: 5000, height: 5000 })).toBe(1);
	});
	it.each([
		[0, 500],
		[500, 0],
		[-1, -1],
		[Number.NaN, 500],
	])("unmeasured avail (%s, %s) -> 1", (width, height) => {
		expect(fitScale(frame, { width, height })).toBe(1);
	});
});

describe("deviceParams", () => {
	it("is empty without dpr", () => {
		expect(deviceParams(undefined)).toEqual([]);
		expect(deviceParams({ preset: "iphone-14" })).toEqual([]);
	});
	it("emits dpr when set", () => {
		expect(deviceParams({ preset: "iphone-14", dpr: 2 })).toEqual([["dpr", "2"]]);
	});
});

describe("deviceLabel", () => {
	it.each<[DesignDevice | undefined, string]>([
		[undefined, "None"],
		[{ preset: "none" }, "None"],
		[{ preset: "iphone-14" }, "iPhone 14 · 390×844"],
		[{ preset: "iphone-14", landscape: true }, "iPhone 14 · 844×390"],
		[{ preset: "custom", width: 500, height: 700 }, "Custom · 500×700"],
		[{ preset: "ipad-air", dpr: 2 }, "iPad Air · 820×1180 · 2×"],
	])("%j -> %s", (d, want) => {
		expect(deviceLabel(d)).toBe(want);
	});
});

describe("portraitSize", () => {
	it("is portrait whatever the orientation", () => {
		expect(portraitSize({ preset: "iphone-14", landscape: true })).toEqual({
			width: 390,
			height: 844,
		});
		expect(portraitSize({ preset: "custom", width: 500, height: 700, landscape: true })).toEqual({
			width: 500,
			height: 700,
		});
		expect(portraitSize(undefined)).toEqual({ width: 390, height: 844 });
	});
});
