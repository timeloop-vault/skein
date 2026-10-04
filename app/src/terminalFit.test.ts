import type { FitAddon } from "@xterm/addon-fit";
import type { Terminal } from "@xterm/xterm";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clipSafeCols, fitTerminal } from "./terminalFit.ts";

describe("clipSafeCols", () => {
	it("drops a column at fs 12 only when the drift needs it", () => {
		// 275 * 7.2 = 1980; drift adds 275/64 = 4.3 px.
		expect(clipSafeCols(275, 7.2, 1980)).toBe(274);
		expect(clipSafeCols(275, 7.2, 1985)).toBe(275);
	});
	it("flips exactly at 275 * (7.2 + 1/64) = 1984.296875", () => {
		expect(clipSafeCols(275, 7.2, 1984.3)).toBe(275);
		expect(clipSafeCols(275, 7.2, 1984.29)).toBe(274);
	});
	it("drops several columns when needed", () => {
		// 96 * 7.203125 = 692.7 > 686; 95 * 7.203125 = 685.484375 <= 686.
		expect(clipSafeCols(100, 7.2, 686)).toBe(95);
	});
	it("keeps proposedCols when there is slack (fs 13)", () => {
		expect(clipSafeCols(100, 7.8, 790)).toBe(100);
	});
	it("drops one column on an exact fit", () => {
		expect(clipSafeCols(100, 7.2, 720)).toBe(99);
	});
	it("never goes below 2 or above proposedCols", () => {
		expect(clipSafeCols(10, 7.2, 1)).toBe(2);
		expect(clipSafeCols(1, 7.2, 1000)).toBe(2);
		expect(clipSafeCols(50, 7.2, 10000)).toBe(50);
	});
	it("leaves small cols with normal slack unchanged", () => {
		expect(clipSafeCols(49, 7.2, 360)).toBe(49);
	});
});

describe("fitTerminal", () => {
	afterEach(() => vi.unstubAllGlobals());

	function setup(opts: {
		proposed: { cols: number; rows: number } | undefined;
		cols: number;
		rows: number;
		parentWidth: number;
	}) {
		const resize = vi.fn();
		const clear = vi.fn();
		const parent = { id: "parent" };
		const element = { parentElement: parent };
		const term = {
			_core: { _renderService: { dimensions: { css: { cell: { width: 7.2 } } }, clear } },
			element,
			options: { scrollback: 1000, overviewRuler: { width: 14 } },
			rows: opts.rows,
			cols: opts.cols,
			resize,
		} as unknown as Terminal;
		const fit = { proposeDimensions: () => opts.proposed } as unknown as FitAddon;
		vi.stubGlobal("window", {
			getComputedStyle: (el: unknown) => ({
				getPropertyValue: (name: string) =>
					el === parent ? `${opts.parentWidth}px` : name.startsWith("padding") ? "0px" : "",
			}),
		});
		return { term, fit, resize, clear };
	}

	it("returns early without resize when proposeDimensions is undefined", () => {
		const { term, fit, resize } = setup({
			proposed: undefined,
			cols: 80,
			rows: 24,
			parentWidth: 900,
		});
		fitTerminal(term, fit);
		expect(resize).not.toHaveBeenCalled();
	});
	it("passes rows through and reduces cols when drift would reach the scrollbar", () => {
		// available = 1994 - 14 = 1980; 275 cols need 1984.3.
		const { term, fit, resize, clear } = setup({
			proposed: { cols: 275, rows: 30 },
			cols: 80,
			rows: 24,
			parentWidth: 1994,
		});
		fitTerminal(term, fit);
		expect(clear).toHaveBeenCalledOnce();
		expect(resize).toHaveBeenCalledWith(274, 30);
	});
	it("does not resize when nothing changed", () => {
		const { term, fit, resize } = setup({
			proposed: { cols: 100, rows: 24 },
			cols: 100,
			rows: 24,
			parentWidth: 1000,
		});
		fitTerminal(term, fit);
		expect(resize).not.toHaveBeenCalled();
	});
});
