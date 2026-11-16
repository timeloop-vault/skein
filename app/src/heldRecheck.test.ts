import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createHeldRecheck, HELD_RECHECK_MS } from "./heldRecheck.ts";

describe("createHeldRecheck", () => {
	beforeEach(() => {
		vi.useFakeTimers();
	});
	afterEach(() => {
		vi.useRealTimers();
	});

	function setup(registered = true) {
		const checks: string[] = [];
		const state = { registered };
		const r = createHeldRecheck({
			check: (id) => checks.push(id),
			isRegistered: () => state.registered,
		});
		return { r, checks, state };
	}

	it("checks at once when the hold starts, then every 2 s", () => {
		const { r, checks } = setup();
		r.setHeld("h", true);
		expect(checks).toEqual(["h"]);
		vi.advanceTimersByTime(HELD_RECHECK_MS * 3);
		expect(checks).toEqual(["h", "h", "h", "h"]);
	});

	it("does not start a second timer for a repeated held=true", () => {
		const { r, checks } = setup();
		r.setHeld("h", true);
		r.setHeld("h", true);
		vi.advanceTimersByTime(HELD_RECHECK_MS);
		expect(checks).toHaveLength(2);
	});

	it("stops when the hold ends", () => {
		const { r, checks } = setup();
		r.setHeld("h", true);
		r.setHeld("h", false);
		vi.advanceTimersByTime(HELD_RECHECK_MS * 3);
		expect(checks).toEqual(["h"]);
		expect(r.active()).toEqual([]);
	});

	it("stops, without a check, once the harness unregisters", () => {
		const { r, checks, state } = setup();
		r.setHeld("h", true);
		state.registered = false;
		vi.advanceTimersByTime(HELD_RECHECK_MS * 3);
		expect(checks).toEqual(["h"]);
		expect(r.active()).toEqual([]);
	});

	it("tracks harnesses independently and dispose clears every timer", () => {
		const { r, checks } = setup();
		r.setHeld("a", true);
		r.setHeld("b", true);
		r.setHeld("a", false);
		vi.advanceTimersByTime(HELD_RECHECK_MS);
		expect(checks).toEqual(["a", "b", "b"]);
		r.dispose();
		vi.advanceTimersByTime(HELD_RECHECK_MS * 3);
		expect(checks).toHaveLength(3);
		expect(vi.getTimerCount()).toBe(0);
	});
});
