import { describe, expect, it, vi } from "vitest";
import type { RoomMutation, RoomsApplyOutcome } from "./roomMutations.ts";
import { createRoomsSync } from "./roomsSync.ts";
import type { Room } from "./types.ts";

const room = (id: string, name = id): Room =>
	({
		id,
		name,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: [],
		activeHarnessId: "",
	}) as unknown as Room;
const tick = () => new Promise<void>((r) => setTimeout(r, 0));

function setup() {
	const calls: { mutations: RoomMutation[]; resolve: () => void }[] = [];
	const flushDaemon = vi.fn(async () => {});
	const sync = createRoomsSync({
		clientId: "me",
		apply: (mutations) =>
			new Promise<RoomsApplyOutcome>((res) => {
				calls.push({ mutations, resolve: () => res({ skipped: [] }) });
			}),
		flushDaemon,
		onError: () => {},
		setTimer: () => 0,
		clearTimer: () => {},
	});
	return { sync, calls, flushDaemon };
}

describe("createRoomsSync: drain (#731)", () => {
	it("resolves at once with nothing pending, and never flushes the daemon", async () => {
		const { sync, calls, flushDaemon } = setup();
		sync.setBaseline([room("a")]);
		await sync.drain();
		expect(calls).toHaveLength(0);
		expect(flushDaemon).not.toHaveBeenCalled();
	});

	it("sends the latest diff when none is in flight", async () => {
		const { sync, calls, flushDaemon } = setup();
		sync.setBaseline([room("a")]);
		// update() would pump itself; drain must also cover an update made in
		// the same tick the drain starts (child effect before parent effect).
		const p = sync.drain();
		sync.update([room("a", "renamed")]);
		await tick();
		expect(calls).toHaveLength(1);
		calls[0]?.resolve();
		await p;
		expect(flushDaemon).not.toHaveBeenCalled();
	});

	it("waits for the in-flight call, then sends the newer diff, then resolves", async () => {
		const { sync, calls, flushDaemon } = setup();
		sync.setBaseline([room("a")]);
		sync.update([room("a", "one")]);
		expect(calls).toHaveLength(1);
		sync.update([room("a", "two")]); // swallowed by the running pump
		let done = false;
		const p = sync.drain().then(() => {
			done = true;
		});
		await tick();
		expect(done).toBe(false);
		expect(calls).toHaveLength(1);
		calls[0]?.resolve();
		await tick();
		// the run loop itself picks up the newer snapshot
		expect(calls).toHaveLength(2);
		expect(done).toBe(false);
		calls[1]?.resolve();
		await p;
		expect(done).toBe(true);
		expect(calls).toHaveLength(2);
		expect(flushDaemon).not.toHaveBeenCalled();
	});
});
