import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
	clockOffset,
	debounce,
	effectiveNow,
	isRectVisible,
	isSentinelPosition,
	toLogicalGeometry,
} from "./popoutWindow.ts";

describe("clock", () => {
	it("advances from the snapshot instant", () => {
		const off = clockOffset(10_000, 500);
		expect(effectiveNow(off, 500)).toBe(10_000);
		expect(effectiveNow(off, 2_500)).toBe(12_000);
	});
});

describe("toLogicalGeometry", () => {
	it("divides by the scale factor", () => {
		expect(toLogicalGeometry({ x: 300, y: 150 }, { width: 1500, height: 900 }, 1.5)).toEqual({
			x: 200,
			y: 100,
			width: 1000,
			height: 600,
		});
	});
	it("keeps negative positions and guards a zero scale", () => {
		expect(toLogicalGeometry({ x: -200, y: 0 }, { width: 800, height: 600 }, 0)).toEqual({
			x: -200,
			y: 0,
			width: 800,
			height: 600,
		});
	});
});

describe("isRectVisible", () => {
	const mon = [
		{ x: 0, y: 0, width: 1920, height: 1040 },
		{ x: 1920, y: 0, width: 1280, height: 1000 },
	];
	it("accepts a rect on a monitor", () => {
		expect(isRectVisible({ x: 100, y: 100, width: 980, height: 640 }, mon)).toBe(true);
	});
	it("accepts a rect on the second monitor", () => {
		expect(isRectVisible({ x: 2500, y: 50, width: 600, height: 400 }, mon)).toBe(true);
	});
	it("rejects a rect on a disconnected monitor", () => {
		expect(isRectVisible({ x: 5000, y: 100, width: 980, height: 640 }, mon)).toBe(false);
	});
	it("rejects a rect with only a sliver visible", () => {
		expect(isRectVisible({ x: -900, y: 100, width: 980, height: 640 }, mon)).toBe(false);
		expect(isRectVisible({ x: 100, y: 1000, width: 980, height: 640 }, mon)).toBe(false);
	});
	it("rejects the minimized sentinel and no monitors", () => {
		expect(isRectVisible({ x: -32000, y: -32000, width: 980, height: 640 }, mon)).toBe(false);
		expect(isRectVisible({ x: 0, y: 0, width: 980, height: 640 }, [])).toBe(false);
		expect(isSentinelPosition({ x: -30000, y: 0 })).toBe(true);
		expect(isSentinelPosition({ x: -200, y: 0 })).toBe(false);
	});
});

describe("debounce", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());
	it("fires once after the quiet period", () => {
		const fn = vi.fn();
		const d = debounce(fn, 500);
		d();
		vi.advanceTimersByTime(300);
		d();
		vi.advanceTimersByTime(400);
		expect(fn).not.toHaveBeenCalled();
		vi.advanceTimersByTime(100);
		expect(fn).toHaveBeenCalledTimes(1);
	});
	it("cancel drops the pending call", () => {
		const fn = vi.fn();
		const d = debounce(fn, 100);
		d();
		d.cancel();
		vi.advanceTimersByTime(500);
		expect(fn).not.toHaveBeenCalled();
	});
});
