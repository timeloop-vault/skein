import { describe, expect, it } from "vitest";
import { applyRoomChanges } from "./roomChanges.ts";
import type { Harness, Room } from "./types.ts";

const harness = (id: string, extra: Record<string, unknown> = {}): Harness =>
	({ id, kind: "shell", name: id, ...extra }) as unknown as Harness;
const room = (
	id: string,
	hs: Harness[] = [harness("h1")],
	extra: Record<string, unknown> = {},
): Room =>
	({
		id,
		name: id,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: hs,
		activeHarnessId: hs[0]?.id ?? "",
		...extra,
	}) as unknown as Room;
const ids = (rs: Room[]) => rs.map((r) => r.id);

describe("applyRoomChanges", () => {
	it("roomPut replaces in place", () => {
		const out = applyRoomChanges(
			[room("a"), room("b"), room("c")],
			[{ type: "roomPut", room: room("b", [harness("h1")], { name: "B2" }) }],
		);
		expect(ids(out)).toEqual(["a", "b", "c"]);
		expect(out[1]?.name).toBe("B2");
	});

	it("roomPut replaces: local-only keys are gone, cleared fields are cleared", () => {
		const local = room("a", [harness("h1", { localOnly: 7 })], {
			localRoomOnly: "x",
			archived: 5,
		});
		const remote = room("a", [harness("h1", { name: "new" })], { name: "A2" });
		const out = applyRoomChanges([local], [{ type: "roomPut", room: remote }]);
		const r = out[0] as unknown as Record<string, unknown>;
		expect(r.localRoomOnly).toBeUndefined();
		expect(r.archived).toBeUndefined();
		expect(r.name).toBe("A2");
		const h = out[0]?.harnesses[0] as unknown as Record<string, unknown>;
		expect(h.localOnly).toBeUndefined();
		expect(h.name).toBe("new");
	});

	it("roomPut adopts the remote harness list (adds, drops, orders)", () => {
		const local = room("a", [harness("x"), harness("y")]);
		const remote = room("a", [harness("z"), harness("y")]);
		const out = applyRoomChanges([local], [{ type: "roomPut", room: remote }]);
		expect(out[0]?.harnesses.map((h) => h.id)).toEqual(["z", "y"]);
	});

	it("roomPut of an unknown id appends", () => {
		const out = applyRoomChanges([room("a")], [{ type: "roomPut", room: room("n") }]);
		expect(ids(out)).toEqual(["a", "n"]);
	});

	it("roomRemoved removes, and is a no-op for an absent id", () => {
		const start = [room("a"), room("b")];
		expect(ids(applyRoomChanges(start, [{ type: "roomRemoved", roomId: "a" }]))).toEqual(["b"]);
		const out = applyRoomChanges(start, [{ type: "roomRemoved", roomId: "zzz" }]);
		expect(out).toBe(start);
	});

	it("roomsOrder ignores unknown ids and keeps unlisted rooms after the listed ones", () => {
		const out = applyRoomChanges(
			[room("a"), room("b"), room("c"), room("d")],
			[{ type: "roomsOrder", roomIds: ["c", "ghost", "a"] }],
		);
		expect(ids(out)).toEqual(["c", "a", "b", "d"]);
	});

	it("is idempotent", () => {
		const changes = [
			{ type: "roomPut" as const, room: room("b", [harness("h1")], { name: "B2" }) },
			{ type: "roomPut" as const, room: room("n") },
			{ type: "roomRemoved" as const, roomId: "c" },
			{ type: "roomsOrder" as const, roomIds: ["n", "b", "a"] },
		];
		const once = applyRoomChanges([room("a"), room("b"), room("c")], changes);
		const twice = applyRoomChanges(once, changes);
		expect(twice).toEqual(once);
		expect(twice).toBe(once);
	});

	it("returns the same array when nothing changed", () => {
		const a = room("a");
		const b = room("b");
		const start = [a, b];
		const out = applyRoomChanges(start, [
			{ type: "roomPut", room: structuredClone(a) },
			{ type: "roomsOrder", roomIds: ["a", "b"] },
			{ type: "roomRemoved", roomId: "nope" },
		]);
		expect(out).toBe(start);
		expect(out[0]).toBe(a);
	});

	it("keeps the reference of untouched rooms", () => {
		const a = room("a");
		const b = room("b");
		const c = room("c");
		const out = applyRoomChanges(
			[a, b, c],
			[{ type: "roomPut", room: room("b", [harness("h1")], { name: "B2" }) }],
		);
		expect(out[0]).toBe(a);
		expect(out[1]).not.toBe(b);
		expect(out[2]).toBe(c);
	});

	it("a roomPut lacking optional fields clears them (room and harness)", () => {
		const local = room("a", [harness("h1", { sessionId: "s1" })], {
			archived: 5,
			closedBy: { roomId: "x" },
		});
		const remote = room("a", [harness("h1")]);
		const out = applyRoomChanges([local], [{ type: "roomPut", room: remote }]);
		const r = out[0] as unknown as Record<string, unknown>;
		expect(r.archived).toBeUndefined();
		expect(r.closedBy).toBeUndefined();
		const h = out[0]?.harnesses[0] as unknown as Record<string, unknown> | undefined;
		expect(h).toBeDefined();
		expect(h?.sessionId).toBeUndefined();
	});

	it("the local spawnGen survives a roomPut of that harness", () => {
		const local = room("a", [harness("h1", { spawnGen: 4 })]);
		const remote = room("a", [harness("h1", { name: "new" })]);
		const out = applyRoomChanges([local], [{ type: "roomPut", room: remote }]);
		const h = out[0]?.harnesses[0] as unknown as Record<string, unknown>;
		expect(h.name).toBe("new");
		expect(h.spawnGen).toBe(4);
	});

	it("a roomPut equal apart from spawnGen keeps the room and harness references", () => {
		const local = room("a", [harness("h1", { spawnGen: 4 })]);
		const remote = room("a", [harness("h1")]);
		const start = [local];
		const out = applyRoomChanges(start, [{ type: "roomPut", room: remote }]);
		expect(out).toBe(start);
		expect(out[0]).toBe(local);
		expect(out[0]?.harnesses[0]).toBe(local.harnesses[0]);
	});

	it("keeps harness references that did not change within a put room", () => {
		const h1 = harness("h1");
		const local = room("a", [h1, harness("h2")]);
		const remote = room("a", [harness("h1"), harness("h2", { name: "new" })]);
		const out = applyRoomChanges([local], [{ type: "roomPut", room: remote }]);
		expect(out[0]?.harnesses[0]).toBe(h1);
	});
});
