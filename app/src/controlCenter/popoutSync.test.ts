import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CcSnapshotPayload } from "./popoutProtocol.ts";
import { createThrottle, NOW_REFRESH_MS, snapshotChanged, toggleAction } from "./popoutSync.ts";

describe("createThrottle", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("sends the first push at once and collapses the rest to the newest", () => {
		const send = vi.fn();
		const t = createThrottle<number>(send, 250);
		t.push(1);
		t.push(2);
		t.push(3);
		expect(send.mock.calls).toEqual([[1]]);
		vi.advanceTimersByTime(250);
		expect(send.mock.calls).toEqual([[1], [3]]);
		vi.advanceTimersByTime(1000);
		expect(send).toHaveBeenCalledTimes(2);
	});

	it("sends immediately again once the window has passed", () => {
		const send = vi.fn();
		const t = createThrottle<number>(send, 250);
		t.push(1);
		vi.advanceTimersByTime(300);
		t.push(2);
		expect(send.mock.calls).toEqual([[1], [2]]);
	});

	it("flush sends the pending value now; cancel drops it", () => {
		const send = vi.fn();
		const t = createThrottle<number>(send, 250);
		t.push(1);
		t.push(2);
		t.flush();
		expect(send.mock.calls).toEqual([[1], [2]]);
		t.push(3);
		t.cancel();
		vi.advanceTimersByTime(1000);
		expect(send).toHaveBeenCalledTimes(2);
	});
});

describe("snapshotChanged", () => {
	const snap = (now: number, activeRoomId: string | null = "a"): CcSnapshotPayload => ({
		sections: [],
		activeRoomId,
		now,
	});

	it("always sends the first", () => {
		expect(snapshotChanged(null, snap(0)).send).toBe(true);
	});

	it("drops an identical snapshot, but refreshes `now` eventually", () => {
		const first = snapshotChanged(null, snap(0));
		const prev = { json: first.json, now: 0 };
		expect(snapshotChanged(prev, snap(1000)).send).toBe(false);
		expect(snapshotChanged(prev, snap(NOW_REFRESH_MS)).send).toBe(true);
	});

	it("sends when the active room changes", () => {
		const first = snapshotChanged(null, snap(0));
		expect(snapshotChanged({ json: first.json, now: 0 }, snap(1, "b")).send).toBe(true);
	});
});

describe("toggleAction", () => {
	it("raises when popped out, toggles otherwise", () => {
		expect(toggleAction(true)).toBe("raise");
		expect(toggleAction(false)).toBe("toggle");
	});
});
