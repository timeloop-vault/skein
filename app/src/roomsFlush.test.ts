import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { drainRoomsBounded, registerRoomsDrainer, registerRoomsFlusher } from "./roomsFlush.ts";

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
	vi.useRealTimers();
	registerRoomsDrainer(null);
	registerRoomsFlusher(null);
});

describe("drainRoomsBounded", () => {
	it("is a no-op without a drainer", async () => {
		await expect(drainRoomsBounded()).resolves.toBeUndefined();
	});

	it("calls the drainer, never the flusher, and resolves when it does", async () => {
		const drain = vi.fn(async () => {});
		const flush = vi.fn(async () => {});
		registerRoomsDrainer(drain);
		registerRoomsFlusher(flush);
		await drainRoomsBounded();
		expect(drain).toHaveBeenCalledTimes(1);
		expect(flush).not.toHaveBeenCalled();
	});

	it("gives up after the timeout without rejecting", async () => {
		registerRoomsDrainer(() => new Promise<void>(() => {}));
		let done = false;
		const p = drainRoomsBounded(500).then(() => {
			done = true;
		});
		await vi.advanceTimersByTimeAsync(499);
		expect(done).toBe(false);
		await vi.advanceTimersByTimeAsync(1);
		await p;
		expect(done).toBe(true);
	});

	it("uses 3000 ms by default", async () => {
		registerRoomsDrainer(() => new Promise<void>(() => {}));
		let done = false;
		const p = drainRoomsBounded().then(() => {
			done = true;
		});
		await vi.advanceTimersByTimeAsync(2999);
		expect(done).toBe(false);
		await vi.advanceTimersByTimeAsync(1);
		await p;
		expect(done).toBe(true);
	});

	it("logs and swallows a drainer rejection", async () => {
		const err = vi.spyOn(console, "error").mockImplementation(() => {});
		registerRoomsDrainer(async () => {
			throw new Error("x");
		});
		await expect(drainRoomsBounded()).resolves.toBeUndefined();
		expect(err).toHaveBeenCalled();
		err.mockRestore();
	});

	it("clears its timer when the drainer wins", async () => {
		registerRoomsDrainer(async () => {});
		await drainRoomsBounded();
		expect(vi.getTimerCount()).toBe(0);
	});
});
