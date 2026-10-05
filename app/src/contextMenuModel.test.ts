import { describe, expect, it } from "vitest";
import { clampMenuPosition, nextMenuIndex } from "./contextMenuModel.ts";

const view = { width: 800, height: 600 };
const menu = { width: 200, height: 100 };

describe("clampMenuPosition", () => {
	it("keeps the cursor position when it fits", () => {
		expect(clampMenuPosition({ x: 50, y: 60 }, menu, view)).toEqual({ x: 50, y: 60 });
	});
	it("pushes back from the right and bottom edges", () => {
		expect(clampMenuPosition({ x: 790, y: 590 }, menu, view)).toEqual({ x: 596, y: 496 });
	});
	it("never goes past the left/top edge, even when oversized", () => {
		expect(clampMenuPosition({ x: -5, y: -5 }, menu, view)).toEqual({ x: 4, y: 4 });
		expect(clampMenuPosition({ x: 10, y: 10 }, { width: 900, height: 700 }, view)).toEqual({
			x: 4,
			y: 4,
		});
	});
});

describe("nextMenuIndex", () => {
	it("starts at the ends from nothing focused", () => {
		expect(nextMenuIndex("ArrowDown", -1, 3)).toBe(0);
		expect(nextMenuIndex("ArrowUp", -1, 3)).toBe(2);
	});
	it("wraps", () => {
		expect(nextMenuIndex("ArrowDown", 2, 3)).toBe(0);
		expect(nextMenuIndex("ArrowUp", 0, 3)).toBe(2);
	});
	it("handles Home/End, other keys and empty menus", () => {
		expect(nextMenuIndex("Home", 2, 3)).toBe(0);
		expect(nextMenuIndex("End", 0, 3)).toBe(2);
		expect(nextMenuIndex("a", 0, 3)).toBeNull();
		expect(nextMenuIndex("ArrowDown", -1, 0)).toBeNull();
	});
});
