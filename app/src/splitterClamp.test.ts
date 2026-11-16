import { describe, expect, it } from "vitest";
import { clampSplit } from "./splitterClamp.ts";

describe("clampSplit", () => {
	it("keeps a size that fits", () => {
		expect(clampSplit(640, 1600, 320, 320)).toBe(640);
	});
	it("clamps a size that is too wide", () => {
		expect(clampSplit(640, 800, 320, 320)).toBe(480);
	});
	it("raises a size below minFirst", () => {
		expect(clampSplit(100, 1600, 320, 320)).toBe(320);
	});
	it("lets the second minimum win when both minimums do not fit", () => {
		expect(clampSplit(640, 500, 320, 320)).toBe(180);
		expect(clampSplit(640, 300, 320, 320)).toBe(0);
	});
	it("passes size through when the container is unmeasured", () => {
		expect(clampSplit(640, 0, 320, 320)).toBe(640);
	});
	it("leaves the second pane at least minSecond after the divider", () => {
		const result = clampSplit(640, 800, 320, 320, 4);
		expect(result).toBe(476);
		expect(800 - result - 4).toBeGreaterThanOrEqual(320);
	});
	it("accounts for the divider when the minimums do not fit", () => {
		expect(clampSplit(640, 500, 320, 320, 4)).toBe(176);
		expect(clampSplit(640, 320, 320, 320, 4)).toBe(0);
	});
});
