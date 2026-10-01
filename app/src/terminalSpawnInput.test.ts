import { describe, expect, it } from "vitest";
import { readVisibleScreen, type ScreenTerm } from "./terminalSpawnInput.ts";

function fakeTerm(lines: (string[] | null)[], cols: number): ScreenTerm {
	return {
		rows: lines.length,
		cols,
		buffer: {
			active: {
				baseY: 2,
				cursorX: 1,
				cursorY: 0,
				getNullCell: () => ({}) as never,
				getLine: (y: number) => {
					const l = lines[y - 2];
					if (!l) return undefined;
					return {
						getCell: (x: number) => {
							const ch = l[x];
							if (ch === undefined) return undefined;
							return { getChars: () => ch, isDim: () => (ch === "d" ? 1 : 0) };
						},
					} as never;
				},
			},
		},
	};
}

describe("readVisibleScreen", () => {
	it("reads cells from baseY and reports the cursor", () => {
		const s = readVisibleScreen(fakeTerm([["a", "d"]], 2));
		expect(s).toEqual({
			rows: [
				[
					{ ch: "a", dim: false },
					{ ch: "d", dim: true },
				],
			],
			cursorX: 1,
			cursorY: 0,
		});
	});
	it("is null when a line is missing", () => {
		expect(readVisibleScreen(fakeTerm([["a"], null], 1))).toBeNull();
	});
	it("is null when a cell is missing", () => {
		expect(readVisibleScreen(fakeTerm([["a"]], 2))).toBeNull();
	});
});
