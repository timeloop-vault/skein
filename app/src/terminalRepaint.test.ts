import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { forceRepaint, REPAINT_DELAY_MS, repaintPty } from "./terminalRepaint.ts";

beforeEach(() => {
	vi.useFakeTimers();
	invoke.mockReset();
});
afterEach(() => vi.useRealTimers());

describe("forceRepaint", () => {
	it("shrinks one row, then restores the current size after the delay", () => {
		let size = { rows: 24, cols: 80 };
		const resize = vi.fn();
		forceRepaint({ size: () => size, resize });
		expect(resize).toHaveBeenCalledTimes(1);
		expect(resize).toHaveBeenLastCalledWith(23, 80);
		vi.advanceTimersByTime(REPAINT_DELAY_MS - 1);
		expect(resize).toHaveBeenCalledTimes(1);
		size = { rows: 30, cols: 100 }; // a real resize in between wins
		vi.advanceTimersByTime(1);
		expect(resize).toHaveBeenCalledTimes(2);
		expect(resize).toHaveBeenLastCalledWith(30, 100);
	});

	it("cancel while the restore is pending restores at once, and only once", () => {
		const resize = vi.fn();
		const cancel = forceRepaint({ size: () => ({ rows: 10, cols: 40 }), resize });
		cancel();
		expect(resize).toHaveBeenCalledTimes(2);
		expect(resize).toHaveBeenLastCalledWith(10, 40);
		vi.advanceTimersByTime(1000);
		cancel();
		expect(resize).toHaveBeenCalledTimes(2);
	});

	it("cancel after the restore ran sends nothing more", () => {
		const resize = vi.fn();
		const cancel = forceRepaint({ size: () => ({ rows: 10, cols: 40 }), resize });
		vi.advanceTimersByTime(REPAINT_DELAY_MS);
		cancel();
		expect(resize).toHaveBeenCalledTimes(2);
	});

	it("does nothing for a one-row terminal", () => {
		const resize = vi.fn();
		const cancel = forceRepaint({ size: () => ({ rows: 1, cols: 40 }), resize });
		vi.advanceTimersByTime(1000);
		expect(resize).not.toHaveBeenCalled();
		expect(() => cancel()).not.toThrow();
	});

	it("honours a custom delay", () => {
		const resize = vi.fn();
		forceRepaint({ size: () => ({ rows: 5, cols: 5 }), resize, delayMs: 500 });
		vi.advanceTimersByTime(499);
		expect(resize).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(1);
		expect(resize).toHaveBeenCalledTimes(2);
	});
});

describe("repaintPty", () => {
	it("resizes the pty by id, one row fewer and back", () => {
		const cancel = repaintPty({ rows: 20, cols: 60 }, "p1");
		expect(invoke).toHaveBeenCalledTimes(1);
		expect(invoke).toHaveBeenLastCalledWith("pty_resize", { id: "p1", rows: 19, cols: 60 });
		vi.advanceTimersByTime(REPAINT_DELAY_MS);
		expect(invoke).toHaveBeenCalledTimes(2);
		expect(invoke).toHaveBeenLastCalledWith("pty_resize", { id: "p1", rows: 20, cols: 60 });
		cancel();
	});

	it("cancel sends the restoring resize immediately, not later", () => {
		const cancel = repaintPty({ rows: 20, cols: 60 }, "p1");
		cancel();
		expect(invoke).toHaveBeenCalledTimes(2);
		expect(invoke).toHaveBeenLastCalledWith("pty_resize", { id: "p1", rows: 20, cols: 60 });
		vi.advanceTimersByTime(1000);
		expect(invoke).toHaveBeenCalledTimes(2);
	});
});
