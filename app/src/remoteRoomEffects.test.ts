import { describe, expect, it } from "vitest";
import { goneHarnessIds, repairedActiveRoomId } from "./remoteRoomEffects.ts";
import type { Harness, Room } from "./types.ts";

function room(id: string, hs: string[] = [], opts: Partial<Room> = {}): Room {
	return {
		id,
		name: id,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: hs.map((h) => ({ id: h }) as Harness),
		activeHarnessId: hs[0] ?? "",
		...opts,
	};
}

describe("repairedActiveRoomId", () => {
	const none = new Map<string, string>();

	it("leaves an open active room alone", () => {
		expect(repairedActiveRoomId([room("a"), room("b")], "a", none)).toBeNull();
	});

	it("leaves an empty selection alone", () => {
		expect(repairedActiveRoomId([room("a")], "", none)).toBeNull();
	});

	it("moves off a room that was archived remotely", () => {
		const rooms = [room("a", [], { archived: 5 }), room("b")];
		expect(repairedActiveRoomId(rooms, "a", none)).toBe("b");
	});

	it("moves off a room that was removed", () => {
		expect(repairedActiveRoomId([room("b")], "a", none)).toBe("b");
	});

	it("picks the neighbour the user's own close would pick", () => {
		const rooms = [room("a"), room("b", [], { archived: 5 }), room("c")];
		expect(repairedActiveRoomId(rooms, "b", none)).toBe("a");
	});

	it("returns empty when no open room is left", () => {
		expect(repairedActiveRoomId([room("a", [], { archived: 1 })], "a", none)).toBe("");
	});
});

describe("goneHarnessIds", () => {
	it("lists a removed harness", () => {
		const prev = [room("a", ["h1", "h2"])];
		const next = [room("a", ["h1"])];
		expect(goneHarnessIds(prev, next)).toEqual(["h2"]);
	});

	it("lists every harness of an archived room", () => {
		const prev = [room("a", ["h1", "h2"]), room("b", ["h3"])];
		const next = [room("a", ["h1", "h2"], { archived: 9 }), room("b", ["h3"])];
		expect(goneHarnessIds(prev, next)).toEqual(["h1", "h2"]);
	});

	it("lists harnesses of a removed room, and nothing when unchanged", () => {
		const prev = [room("a", ["h1"])];
		expect(goneHarnessIds(prev, [])).toEqual(["h1"]);
		expect(goneHarnessIds(prev, prev)).toEqual([]);
	});

	it("does not list harnesses of rooms archived before", () => {
		const prev = [room("a", ["h1"], { archived: 1 })];
		expect(goneHarnessIds(prev, [])).toEqual([]);
	});
});
