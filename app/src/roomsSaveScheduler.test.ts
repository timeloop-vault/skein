import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoomsSaveScheduler, structuralKey } from "./roomsSaveScheduler.ts";
import type { Harness, Room } from "./types.ts";

const harness = (id: string): Harness => ({ id, kind: "shell", name: id }) as unknown as Harness;
const room = (id: string, hs: string[] = ["h1"], extra: Partial<Room> = {}): Room =>
	({
		id,
		name: id,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: hs.map(harness),
		activeHarnessId: hs[0] ?? "",
		...extra,
	}) as unknown as Room;

function setup() {
	const save = vi.fn(async (_r: Room[]) => {});
	const s = createRoomsSaveScheduler({ save, delayMs: 500, maxWaitMs: 2000 });
	return { save, s };
}

beforeEach(() => {
	vi.useFakeTimers();
});
afterEach(() => {
	vi.useRealTimers();
});

describe("structuralKey", () => {
	it("ignores badge/status churn", () => {
		expect(structuralKey([room("a")])).toBe(structuralKey([room("a", ["h1"], { badge: 3 })]));
	});
	it("sees harness order", () => {
		expect(structuralKey([room("a", ["x", "y"])])).not.toBe(structuralKey([room("a", ["y", "x"])]));
	});
});

describe("createRoomsSaveScheduler", () => {
	it("saves the first update immediately", async () => {
		const { save, s } = setup();
		s.update([room("a")]);
		await vi.advanceTimersByTimeAsync(0);
		expect(save).toHaveBeenCalledTimes(1);
		expect(s.pending()).toBe(false);
	});

	it("coalesces non-structural changes into one trailing save", async () => {
		const { save, s } = setup();
		s.update([room("a")]);
		await vi.advanceTimersByTimeAsync(0);
		for (let i = 1; i <= 5; i++) {
			s.update([room("a", ["h1"], { badge: i })]);
			await vi.advanceTimersByTimeAsync(100);
		}
		expect(save).toHaveBeenCalledTimes(1);
		expect(s.pending()).toBe(true);
		await vi.advanceTimersByTimeAsync(500);
		expect(save).toHaveBeenCalledTimes(2);
		expect(save.mock.calls[1]?.[0][0]?.badge).toBe(5);
		expect(s.pending()).toBe(false);
	});

	it.each([
		["add room", [room("a"), room("b")]],
		["remove room", []],
		["archive", [room("a", ["h1"], { archived: 1 })]],
		["retire", [room("a", ["h1"], { archived: 1, retired: 2 })]],
		["add harness", [room("a", ["h1", "h2"])]],
		["remove harness", [room("a", [])]],
		["reorder harness", [room("a", ["h2", "h1"])]],
		["cwd change", [room("a", ["h1"], { cwd: "/x" })]],
	])("saves immediately on %s", async (_n, next) => {
		const { save, s } = setup();
		const base = _n === "reorder harness" ? [room("a", ["h1", "h2"])] : [room("a")];
		s.update(base);
		await vi.advanceTimersByTimeAsync(0);
		s.update(next);
		await vi.advanceTimersByTimeAsync(0);
		expect(save).toHaveBeenCalledTimes(2);
		expect(s.pending()).toBe(false);
	});

	it("honours max wait under a constant stream", async () => {
		const { save, s } = setup();
		s.update([room("a")]);
		await vi.advanceTimersByTimeAsync(0);
		for (let i = 1; i <= 30; i++) {
			s.update([room("a", ["h1"], { badge: i })]);
			await vi.advanceTimersByTimeAsync(100);
		}
		expect(save.mock.calls.length).toBeGreaterThanOrEqual(2);
	});

	it("flush saves the latest rooms and clears pending", async () => {
		const { save, s } = setup();
		s.update([room("a")]);
		await vi.advanceTimersByTimeAsync(0);
		s.update([room("a", ["h1"], { badge: 9 })]);
		expect(s.pending()).toBe(true);
		await s.flush();
		expect(save).toHaveBeenCalledTimes(2);
		expect(save.mock.calls[1]?.[0][0]?.badge).toBe(9);
		expect(s.pending()).toBe(false);
		await vi.advanceTimersByTimeAsync(5000);
		expect(save).toHaveBeenCalledTimes(2);
	});

	it("flush with nothing pending is a no-op", async () => {
		const { save, s } = setup();
		await s.flush();
		s.update([room("a")]);
		await vi.advanceTimersByTimeAsync(0);
		await s.flush();
		expect(save).toHaveBeenCalledTimes(1);
	});

	it("flush swallows save errors", async () => {
		const err = vi.spyOn(console, "error").mockImplementation(() => {});
		const save = vi.fn(async () => {
			throw new Error("boom");
		});
		const s = createRoomsSaveScheduler({ save });
		s.update([room("a")]);
		s.update([room("a", ["h1"], { badge: 1 })]);
		await expect(s.flush()).resolves.toBeUndefined();
		expect(err).toHaveBeenCalled();
		err.mockRestore();
	});

	it("dispose cancels the pending save", async () => {
		const { save, s } = setup();
		s.update([room("a")]);
		await vi.advanceTimersByTimeAsync(0);
		s.update([room("a", ["h1"], { badge: 1 })]);
		s.dispose();
		await vi.advanceTimersByTimeAsync(5000);
		expect(save).toHaveBeenCalledTimes(1);
		expect(s.pending()).toBe(false);
	});

	it("retries a failed save on its own", async () => {
		const err = vi.spyOn(console, "error").mockImplementation(() => {});
		const save = vi
			.fn<(r: Room[]) => Promise<void>>()
			.mockRejectedValueOnce(new Error("boom"))
			.mockResolvedValue(undefined);
		const s = createRoomsSaveScheduler({ save, delayMs: 500, maxWaitMs: 2000 });
		s.update([room("a")]);
		await vi.advanceTimersByTimeAsync(0);
		expect(save).toHaveBeenCalledTimes(1);
		expect(s.pending()).toBe(true);
		await vi.advanceTimersByTimeAsync(2000);
		expect(save).toHaveBeenCalledTimes(2);
		expect(s.pending()).toBe(false);
		err.mockRestore();
	});

	it("issues saves in order, one at a time", async () => {
		const resolvers: Array<() => void> = [];
		const order: string[] = [];
		const save = vi.fn(
			(r: Room[]) =>
				new Promise<void>((res) => {
					order.push(`start:${r.length}`);
					resolvers.push(() => {
						order.push(`end:${r.length}`);
						res();
					});
				}),
		);
		const s = createRoomsSaveScheduler({ save });
		s.update([room("a")]);
		s.update([room("a"), room("b")]);
		await vi.advanceTimersByTimeAsync(0);
		expect(order).toEqual(["start:1"]);
		resolvers[0]?.();
		await vi.advanceTimersByTimeAsync(0);
		expect(order).toEqual(["start:1", "end:1", "start:2"]);
		resolvers[1]?.();
		await s.flush();
	});

	it("drops a debounced save when canSave is false at fire time", async () => {
		const save = vi.fn(async (_r: Room[]) => {});
		let ok = true;
		const s = createRoomsSaveScheduler({ save, canSave: () => ok });
		s.update([room("a")]);
		await vi.advanceTimersByTimeAsync(0);
		s.update([room("a", ["h1"], { badge: 1 })]);
		ok = false;
		await vi.advanceTimersByTimeAsync(1000);
		expect(save).toHaveBeenCalledTimes(1);
		expect(s.pending()).toBe(false);
	});

	it("treats agent, session, repoRoot and branch changes as structural", () => {
		const base = structuralKey([room("a")]);
		expect(structuralKey([room("a", ["h1"], { repoRoot: "/r" })])).not.toBe(base);
		expect(structuralKey([room("a", ["h1"], { branch: "b" })])).not.toBe(base);
		const withH = (patch: Record<string, unknown>) => {
			const r = room("a");
			r.harnesses = r.harnesses.map((h) => ({ ...h, ...patch }));
			return structuralKey([r]);
		};
		expect(withH({ sessionId: "s" })).not.toBe(base);
		expect(withH({ agent: "x" })).not.toBe(base);
		expect(withH({ cwd: "/c" })).not.toBe(base);
	});
});
