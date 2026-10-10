import { describe, expect, it } from "vitest";
import { sgrAfterDecMode, sgrWheelReports, wheelLines } from "./terminalWheel.ts";

const PIXEL = 0;
const LINE = 1;
const PAGE = 2;
const CELL = 17;
const ROWS = 40;

describe("wheelLines", () => {
	it("reports one line per cell height of travel, with no trackpad damping", () => {
		// xterm 6.0.0 scales a <50 px event by 0.3; 34 px must still be 2 lines.
		expect(wheelLines({ lines: 0 }, 34, PIXEL, CELL, ROWS)).toBe(2);
	});

	it("reports every line a large event is worth, not one per event", () => {
		expect(wheelLines({ lines: 0 }, 170, PIXEL, CELL, ROWS)).toBe(10);
	});

	it("carries the fraction until it makes a whole line", () => {
		const carry = { lines: 0 };
		const sent = [5, 5, 5, 5, 5, 5, 5].map((d) => wheelLines(carry, d, PIXEL, CELL, ROWS));
		// 35 px over 17 px cells = 2 lines, landing on the 4th and 7th event.
		expect(sent).toEqual([0, 0, 0, 1, 0, 0, 1]);
	});

	it("scrolls up for a negative delta", () => {
		expect(wheelLines({ lines: 0 }, -51, PIXEL, CELL, ROWS)).toBe(-3);
	});

	it("drops the carry when the direction reverses", () => {
		const carry = { lines: 0 };
		expect(wheelLines(carry, 16, PIXEL, CELL, ROWS)).toBe(0);
		// Without the reset, the 16 px down would swallow this 17 px up.
		expect(wheelLines(carry, -17, PIXEL, CELL, ROWS)).toBe(-1);
	});

	it("keeps the carry across an event with no vertical travel", () => {
		const carry = { lines: 0 };
		wheelLines(carry, 10, PIXEL, CELL, ROWS);
		expect(wheelLines(carry, 0, PIXEL, CELL, ROWS)).toBe(0);
		expect(wheelLines(carry, 8, PIXEL, CELL, ROWS)).toBe(1);
	});

	it("takes line and page deltas at face value", () => {
		expect(wheelLines({ lines: 0 }, 3, LINE, CELL, ROWS)).toBe(3);
		expect(wheelLines({ lines: 0 }, -1, PAGE, CELL, ROWS)).toBe(-ROWS);
	});

	it("reports nothing without a measurable cell height", () => {
		expect(wheelLines({ lines: 0 }, 40, PIXEL, 0, ROWS)).toBe(0);
	});
});

describe("sgrWheelReports", () => {
	it("writes one wheel-down report per line", () => {
		expect(sgrWheelReports(2, 5, 9)).toBe("\x1b[<65;5;9M\x1b[<65;5;9M");
	});

	it("writes wheel-up for negative lines", () => {
		expect(sgrWheelReports(-1, 1, 1)).toBe("\x1b[<64;1;1M");
	});
});

describe("sgrAfterDecMode", () => {
	it("follows DECSET/DECRST 1006", () => {
		expect(sgrAfterDecMode(false, [1006], true)).toBe(true);
		expect(sgrAfterDecMode(true, [1006], false)).toBe(false);
	});

	it("turns on from a combined sequence like Claude Code's", () => {
		expect(sgrAfterDecMode(false, [1000, 1002, 1003, 1006], true)).toBe(true);
	});

	it("treats SGR-pixels (1016) as not SGR, set or reset", () => {
		expect(sgrAfterDecMode(true, [1016], true)).toBe(false);
		expect(sgrAfterDecMode(true, [1016], false)).toBe(false);
	});

	it("ignores unrelated modes", () => {
		expect(sgrAfterDecMode(true, [1049, 25], false)).toBe(true);
		expect(sgrAfterDecMode(false, [1049, 25], true)).toBe(false);
	});
});
