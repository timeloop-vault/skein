import { describe, expect, it } from "vitest";
import type { RoomChange, RoomsChanged } from "./roomMutations.ts";
import { createRoomsSync } from "./roomsSync.ts";
import type { Room } from "./types.ts";

const room = (id: string): Room =>
	({
		id,
		name: id,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: [],
		activeHarnessId: "",
	}) as unknown as Room;
const put = (id: string): RoomChange => ({ type: "roomPut", room: room(id) });
const ev = (changes: RoomChange[], seq?: number): RoomsChanged => ({
	origin: "other",
	changes,
	...(seq === undefined ? {} : { seq }),
});
const ids = (rs: readonly Room[]) => rs.map((r) => r.id);
const make = () =>
	createRoomsSync({
		clientId: "me",
		apply: async () => ({ skipped: [] }),
		flushDaemon: async () => undefined,
	});

describe("roomsSync snapshot watermark (#757)", () => {
	it("does not replay a queued event at or below the snapshot seq", () => {
		const sync = make();
		sync.beginHydrate();
		sync.remote(ev([put("r2")], 5));
		sync.remote(ev([put("r3")], 7));
		const local = [room("r1")];
		expect(sync.finishHydrate([room("r1")], local, 7)).toBe(local);
	});

	it("replays a queued event above the snapshot seq", () => {
		const sync = make();
		sync.beginHydrate();
		sync.remote(ev([put("r2")], 5));
		sync.remote(ev([put("r3")], 8));
		const out = sync.finishHydrate([room("r1")], [room("r1")], 7);
		expect(ids(out)).toEqual(["r1", "r3"]);
	});

	it("drops a stale event delivered after hydrate", () => {
		const sync = make();
		sync.beginHydrate();
		sync.finishHydrate([room("r1")], [room("r1")], 7);
		expect(sync.remote(ev([put("r2")], 7))).toBeNull();
		expect(sync.remote(ev([put("r2")], 3))).toBeNull();
		const up = sync.remote(ev([put("r3")], 8));
		expect(ids(up?.([room("r1")]) ?? [])).toEqual(["r1", "r3"]);
	});

	it("applies an unstamped event as before", () => {
		const sync = make();
		sync.beginHydrate();
		sync.remote(ev([put("r2")]));
		sync.remote(ev([put("r4")], 0));
		const out = sync.finishHydrate([room("r1")], [room("r1")], 7);
		expect(ids(out)).toEqual(["r1", "r2", "r4"]);
		expect(typeof sync.remote(ev([put("r3")]))).toBe("function");
	});

	it("a re-hydrate resets the watermark", () => {
		const sync = make();
		sync.beginHydrate();
		sync.finishHydrate([room("r1")], [room("r1")], 50);
		sync.beginHydrate();
		sync.remote(ev([put("r2")], 10));
		const out = sync.finishHydrate([room("r1")], [room("r1")], 5);
		expect(ids(out)).toEqual(["r1", "r2"]);
	});
});
