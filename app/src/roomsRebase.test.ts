import { describe, expect, it } from "vitest";
import { applyRoomChanges } from "./roomChanges.ts";
import { diffRooms, type RoomChange } from "./roomMutations.ts";
import { applyMutations } from "./roomStoreModel.testutil.ts";
import { rebaseRemote } from "./roomsRebase.ts";
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
const put = (r: Room): RoomChange => ({ type: "roomPut", room: r });
const ids = (rs: readonly Room[]) => rs.map((r) => r.id);
const hids = (r: Room | undefined) => r?.harnesses.map((h) => h.id);

describe("rebaseRemote: room-level fields", () => {
	it("takes the remote value when local equals the old baseline", () => {
		const base = [room("a")];
		const out = rebaseRemote(base, [room("a")], [put(room("a", [harness("h1")], { task: "T" }))]);
		expect(out.local[0]?.task).toBe("T");
	});

	it("keeps a local edit and still takes other remote fields", () => {
		const base = [room("a")];
		const local = [room("a", [harness("h1")], { name: "mine" })];
		const remote = room("a", [harness("h1")], { name: "theirs", task: "T" });
		const out = rebaseRemote(base, local, [put(remote)]);
		expect(out.local[0]?.name).toBe("mine");
		expect(out.local[0]?.task).toBe("T");
	});
});

describe("rebaseRemote: harnesses", () => {
	it("takes the remote harness value when local equals the old baseline", () => {
		const base = [room("a")];
		const remote = room("a", [harness("h1", { name: "new" })]);
		const out = rebaseRemote(base, [room("a")], [put(remote)]);
		expect(out.local[0]?.harnesses[0]?.name).toBe("new");
	});

	it("keeps a locally edited harness over the remote value", () => {
		const base = [room("a")];
		const local = [room("a", [harness("h1", { name: "mine" })])];
		const remote = room("a", [harness("h1", { name: "theirs" })]);
		const out = rebaseRemote(base, local, [put(remote)]);
		expect(out.local[0]?.harnesses[0]?.name).toBe("mine");
	});

	it("keeps a pending local harness add", () => {
		const base = [room("a")];
		const local = [room("a", [harness("h1"), harness("new")])];
		const remote = room("a", [harness("h1")], { task: "T" });
		const out = rebaseRemote(base, local, [put(remote)]);
		expect(hids(out.local[0])).toEqual(["h1", "new"]);
		expect(out.local[0]?.task).toBe("T");
	});

	it("drops a harness the remote dropped when it is not a pending add", () => {
		const base = [room("a", [harness("h1"), harness("h2")])];
		const local = [room("a", [harness("h1"), harness("h2")])];
		const out = rebaseRemote(base, local, [put(room("a", [harness("h1")]))]);
		expect(hids(out.local[0])).toEqual(["h1"]);
	});

	it("adds a harness the remote added", () => {
		const base = [room("a")];
		const out = rebaseRemote(base, [room("a")], [put(room("a", [harness("h1"), harness("r")]))]);
		expect(hids(out.local[0])).toEqual(["h1", "r"]);
	});
});

describe("rebaseRemote: rooms", () => {
	it("keeps a room removed locally (still in the old baseline) removed", () => {
		const base = [room("a"), room("b")];
		const local = [room("a")];
		const out = rebaseRemote(base, local, [put(room("b", [harness("h1")], { task: "T" }))]);
		expect(ids(out.local)).toEqual(["a"]);
		expect(ids(out.baseline)).toEqual(["a", "b"]);
		expect(out.baseline[1]?.task).toBe("T");
	});

	it("appends a room unknown to both baseline and local", () => {
		const out = rebaseRemote([room("a")], [room("a")], [put(room("n"))]);
		expect(ids(out.local)).toEqual(["a", "n"]);
		expect(ids(out.baseline)).toEqual(["a", "n"]);
	});

	it("roomRemoved removes the room locally", () => {
		const base = [room("a"), room("b")];
		const out = rebaseRemote(base, [room("a"), room("b")], [{ type: "roomRemoved", roomId: "a" }]);
		expect(ids(out.local)).toEqual(["b"]);
		expect(ids(out.baseline)).toEqual(["b"]);
	});

	it("keeps a pending local room create", () => {
		const base = [room("a")];
		const local = [room("a"), room("mine")];
		const out = rebaseRemote(base, local, [put(room("a", [harness("h1")], { task: "T" }))]);
		expect(ids(out.local)).toEqual(["a", "mine"]);
	});
});

describe("rebaseRemote: roomsOrder", () => {
	const three = () => [room("a"), room("b"), room("c")];
	const order = (roomIds: string[]): RoomChange => ({ type: "roomsOrder", roomIds });

	it("applies to local when local's order matches the old baseline", () => {
		const out = rebaseRemote(three(), three(), [order(["c", "a", "b"])]);
		expect(ids(out.local)).toEqual(["c", "a", "b"]);
		expect(ids(out.baseline)).toEqual(["c", "a", "b"]);
	});

	it("is ignored when local already reordered the common rooms", () => {
		const local = [room("b"), room("a"), room("c")];
		const out = rebaseRemote(three(), local, [order(["c", "a", "b"])]);
		expect(ids(out.local)).toEqual(["b", "a", "c"]);
		expect(ids(out.baseline)).toEqual(["c", "a", "b"]);
	});
});

describe("rebaseRemote: clears, concurrent edits and local-only keys", () => {
	const field = (r: Room | undefined, k: string) =>
		(r as unknown as Record<string, unknown> | undefined)?.[k];
	const hfield = (r: Room | undefined, k: string) =>
		(r?.harnesses[0] as unknown as Record<string, unknown> | undefined)?.[k];

	it("propagates a remote clear of a room field when there is no pending edit", () => {
		const base = [room("a", [harness("h1")], { archived: 5 })];
		const local = [room("a", [harness("h1")], { archived: 5 })];
		const out = rebaseRemote(base, local, [put(room("a"))]);
		expect(field(out.local[0], "archived")).toBeUndefined();
		expect(field(out.baseline[0], "archived")).toBeUndefined();
		expect(diffRooms(out.baseline, out.local)).toEqual([]);
	});

	it("propagates a remote clear of a harness field when there is no pending edit", () => {
		const base = [room("a", [harness("h1", { sessionId: "s" })])];
		const local = [room("a", [harness("h1", { sessionId: "s" })])];
		const out = rebaseRemote(base, local, [put(room("a"))]);
		expect(hfield(out.local[0], "sessionId")).toBeUndefined();
		expect(hfield(out.baseline[0], "sessionId")).toBeUndefined();
		expect(diffRooms(out.baseline, out.local)).toEqual([]);
	});

	it("keeps a pending room edit of X alongside a remote change of Y", () => {
		const base = [room("a")];
		const local = [room("a", [harness("h1")], { name: "mine" })];
		const out = rebaseRemote(base, local, [put(room("a", [harness("h1")], { task: "T" }))]);
		expect(out.local[0]?.name).toBe("mine");
		expect(out.local[0]?.task).toBe("T");
		expect(diffRooms(out.baseline, out.local).map((m) => m.type)).toEqual(["updateRoom"]);
		expect(out.baseline[0]?.name).toBe("a");
		expect(out.baseline[0]?.task).toBe("T");
	});

	it("keeps a pending harness edit of X alongside a remote change of Y", () => {
		const base = [room("a")];
		const local = [room("a", [harness("h1", { name: "mine" })])];
		const out = rebaseRemote(base, local, [put(room("a", [harness("h1", { sessionId: "s" })]))]);
		expect(hfield(out.local[0], "name")).toBe("mine");
		expect(hfield(out.local[0], "sessionId")).toBe("s");
		expect(diffRooms(out.baseline, out.local).map((m) => m.type)).toEqual(["putHarness"]);
	});

	it("a remote clear of a locally edited room field keeps the local value", () => {
		const base = [room("a", [harness("h1")], { archived: 5 })];
		const local = [room("a", [harness("h1")], { archived: 9 })];
		const out = rebaseRemote(base, local, [put(room("a"))]);
		expect(field(out.local[0], "archived")).toBe(9);
		expect(diffRooms(out.baseline, out.local).map((m) => m.type)).toEqual(["updateRoom"]);
	});

	it("a remote clear of a locally edited harness field keeps the local value", () => {
		const base = [room("a", [harness("h1", { sessionId: "s" })])];
		const local = [room("a", [harness("h1", { sessionId: "mine" })])];
		const out = rebaseRemote(base, local, [put(room("a"))]);
		expect(hfield(out.local[0], "sessionId")).toBe("mine");
		expect(diffRooms(out.baseline, out.local).map((m) => m.type)).toEqual(["putHarness"]);
	});

	it("spawnGen differing from the baseline is no pending edit and keeps the local value", () => {
		const base = [room("a", [harness("h1", { spawnGen: 0 })])];
		const local = [room("a", [harness("h1", { spawnGen: 3 })])];
		const out = rebaseRemote(base, local, [put(room("a", [harness("h1", { name: "new" })]))]);
		expect(hfield(out.local[0], "name")).toBe("new");
		expect(hfield(out.local[0], "spawnGen")).toBe(3);
		expect(diffRooms(out.baseline, out.local)).toEqual([]);
	});

	it("keeps local identity when only spawnGen differs from the remote", () => {
		const base = [room("a", [harness("h1", { spawnGen: 0 })])];
		const local = [room("a", [harness("h1", { spawnGen: 3 })])];
		const out = rebaseRemote(base, local, [put(room("a"))]);
		expect(out.local).toBe(local);
		expect(diffRooms(out.baseline, out.local)).toEqual([]);
	});
});

describe("rebaseRemote: whole-result properties", () => {
	it("without pending edits, local equals applyRoomChanges(local, changes)", () => {
		const base = [room("a"), room("b", [harness("h1"), harness("h2")])];
		const changes: RoomChange[] = [
			put(room("a", [harness("h1", { name: "x" })], { task: "T" })),
			put(room("b", [harness("h2")])),
			put(room("n")),
			{ type: "roomsOrder", roomIds: ["n", "b", "a"] },
		];
		const out = rebaseRemote(base, structuredClone(base), changes);
		expect(out.local).toEqual(applyRoomChanges(structuredClone(base), changes));
		expect(out.baseline).toEqual(applyRoomChanges(base, changes));
	});

	it("preserves identity when nothing changes", () => {
		const base = [room("a"), room("b")];
		const local = [room("a"), room("b")];
		const same = rebaseRemote(base, local, [put(room("a"))]);
		expect(same.local).toBe(local);
		expect(same.local[0]).toBe(local[0]);
		const none = rebaseRemote(base, local, []);
		expect(none.local).toBe(local);
		expect(none.baseline).toBe(base);
	});

	it("preserves identity of untouched rooms and harnesses", () => {
		const base = [room("a"), room("b")];
		const local = [room("a"), room("b")];
		const out = rebaseRemote(base, local, [put(room("a", [harness("h1")], { task: "T" }))]);
		expect(out.local[1]).toBe(local[1]);
		expect(out.local[0]?.harnesses[0]).toBe(local[0]?.harnesses[0]);
	});

	it("leaves diffRooms(newBaseline, newLocal) as exactly the pending local edits", () => {
		const base = [room("a"), room("b"), room("c")];
		const local = [
			room("a", [harness("h1"), harness("new")], { name: "mine" }),
			room("b"),
			room("c"),
			room("added"),
		];
		const out = rebaseRemote(base, local, [
			put(room("a", [harness("h1")], { task: "T" })),
			put(room("b", [harness("h1")], { task: "B2" })),
			{ type: "roomRemoved", roomId: "c" },
		]);
		const direct = diffRooms(out.baseline, out.local);
		// Pending: a's name and new harness, the new room. Not b's remote task, not c.
		expect(direct.map((m) => m.type).sort()).toEqual(
			["createRoom", "putHarness", "updateRoom"].sort(),
		);
		const upd = direct.find((m) => m.type === "updateRoom");
		expect(upd && "room" in upd ? upd.room.name : null).toBe("mine");
		expect(upd && "room" in upd ? upd.room.task : null).toBe("T");
		const create = direct.find((m) => m.type === "createRoom");
		expect(create && "room" in create ? create.room.id : null).toBe("added");
		expect(ids(out.local)).not.toContain("c");
	});
});

describe("rebaseRemote: pending harness edits across a remote roomPut", () => {
	const hs = (...xs: string[]) => xs.map((x) => harness(x));
	const hidsOf = (rs: readonly Room[]) => hids(rs[0]);
	const replay = (base: Room[], local: Room[]) =>
		applyMutations(base, diffRooms(base, local)).rooms;

	it("keeps a pending add in the middle in place and diffs add plus reorder", () => {
		const base = [room("a", hs("h1", "h2"))];
		const local = [room("a", hs("h1", "new", "h2"))];
		const out = rebaseRemote(base, local, [put(room("a", hs("h1", "h2"), { task: "T" }))]);
		expect(hidsOf(out.local)).toEqual(["h1", "new", "h2"]);
		const diff = diffRooms(out.baseline, out.local);
		expect(diff.some((m) => m.type === "putHarness")).toBe(true);
		expect(diff.some((m) => m.type === "reorderHarnesses")).toBe(true);
		expect(hidsOf(replay(out.baseline, out.local))).toEqual(hidsOf(out.local));
	});

	it("keeps a pending harness removal against a remote roomPut", () => {
		const base = [room("a", hs("h1", "h2"))];
		const local = [room("a", hs("h1"))];
		const out = rebaseRemote(base, local, [put(room("a", hs("h1", "h2"), { task: "T" }))]);
		expect(hidsOf(out.local)).toEqual(["h1"]);
		expect(out.local[0]?.task).toBe("T");
		expect(diffRooms(out.baseline, out.local)).toContainEqual({
			type: "removeHarness",
			roomId: "a",
			harnessId: "h2",
		});
	});

	it("remote harness removal vs a local edit of it: the removal wins and converges", () => {
		const base = [room("a", hs("h1", "h2"))];
		const local = [room("a", [harness("h1"), harness("h2", { name: "mine" })])];
		const out = rebaseRemote(base, local, [put(room("a", hs("h1")))]);
		// The local edit is dropped silently: the remote put no longer lists h2.
		expect(hidsOf(out.local)).toEqual(["h1"]);
		expect(hidsOf(replay(out.baseline, out.local))).toEqual(hidsOf(out.local));
		expect(diffRooms(out.baseline, out.local)).toEqual([]);
	});

	it("keeps a pending harness reorder across a remote roomPut", () => {
		const base = [room("a", hs("h1", "h2", "h3"))];
		const local = [room("a", hs("h3", "h1", "h2"))];
		const out = rebaseRemote(base, local, [put(room("a", hs("h1", "h2", "h3"), { task: "T" }))]);
		expect(hidsOf(out.local)).toEqual(["h3", "h1", "h2"]);
		expect(hidsOf(replay(out.baseline, out.local))).toEqual(["h3", "h1", "h2"]);
	});
});
