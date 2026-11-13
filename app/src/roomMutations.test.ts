import { describe, expect, it } from "vitest";
import { applyRoomChanges } from "./roomChanges.ts";
import { deepEqual, diffRooms } from "./roomMutations.ts";
import { applyMutations } from "./roomStoreModel.testutil.ts";
import type { Harness, Room } from "./types.ts";

const harness = (id: string, extra: Partial<Harness> = {}): Harness =>
	({ id, kind: "shell", name: id, ...extra }) as unknown as Harness;
const room = (id: string, hs: string[] = ["h1"], extra: Partial<Room> = {}): Room =>
	({
		id,
		name: id,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: hs.map((h) => harness(h)),
		activeHarnessId: hs[0] ?? "",
		...extra,
	}) as unknown as Room;
const clone = <T>(v: T): T => structuredClone(v);
const types = (ms: { type: string }[]) => ms.map((m) => m.type);

describe("deepEqual", () => {
	it("ignores key order and treats undefined as a missing key", () => {
		expect(deepEqual({ a: 1, b: 2 }, { b: 2, a: 1 })).toBe(true);
		expect(deepEqual({ a: 1, b: undefined }, { a: 1 })).toBe(true);
		expect(deepEqual({ a: 1 }, { a: 2 })).toBe(false);
		expect(deepEqual([1, 2], [2, 1])).toBe(false);
		expect(deepEqual([1], { 0: 1 })).toBe(false);
		expect(deepEqual(null, {})).toBe(false);
	});
});

describe("diffRooms", () => {
	it("gives [] for identical input", () => {
		const a = [room("r1"), room("r2")];
		expect(diffRooms(a, clone(a))).toEqual([]);
	});

	it("emits createRoom for a new room, with no reorderRooms when appended", () => {
		const r2 = room("r2");
		expect(diffRooms([room("r1")], [room("r1"), r2])).toEqual([{ type: "createRoom", room: r2 }]);
	});

	it("emits removeRoom for a removed room", () => {
		expect(diffRooms([room("r1"), room("r2")], [room("r1")])).toEqual([
			{ type: "removeRoom", roomId: "r2" },
		]);
	});

	it("emits exactly one updateRoom and no harness mutations for a room-level change", () => {
		const out = diffRooms([room("r1")], [room("r1", ["h1"], { name: "new", badge: 4 })]);
		expect(types(out)).toEqual(["updateRoom"]);
	});

	it("emits putHarness and no updateRoom for a harness change", () => {
		const next = room("r1");
		next.harnesses = [harness("h1", { name: "renamed" })];
		const out = diffRooms([room("r1")], [next]);
		expect(out).toEqual([{ type: "putHarness", roomId: "r1", harness: next.harnesses[0] }]);
	});

	it("emits putHarness for an added harness", () => {
		const next = room("r1", ["h1", "h2"]);
		expect(diffRooms([room("r1")], [next])).toEqual([
			{ type: "putHarness", roomId: "r1", harness: next.harnesses[1] },
		]);
	});

	it("emits removeHarness for a removed harness", () => {
		expect(diffRooms([room("r1", ["h1", "h2"])], [room("r1")])).toEqual([
			{ type: "removeHarness", roomId: "r1", harnessId: "h2" },
		]);
	});

	it("emits only reorderHarnesses for a harness reorder", () => {
		expect(
			diffRooms(
				[room("r1", ["a", "b", "c"], { activeHarnessId: "a" })],
				[room("r1", ["c", "a", "b"], { activeHarnessId: "a" })],
			),
		).toEqual([{ type: "reorderHarnesses", roomId: "r1", harnessIds: ["c", "a", "b"] }]);
	});

	it("emits only reorderRooms for a room reorder", () => {
		expect(diffRooms([room("a"), room("b")], [room("b"), room("a")])).toEqual([
			{ type: "reorderRooms", roomIds: ["b", "a"] },
		]);
	});

	it("puts reorderRooms last", () => {
		const out = diffRooms(
			[room("a"), room("b")],
			[room("b", ["h1"], { name: "x" }), room("a"), room("c")],
		);
		expect(types(out)).toEqual(["createRoom", "updateRoom", "reorderRooms"]);
	});

	it("skips reference-equal rooms without comparing them", () => {
		const a = room("a");
		const b = room("b");
		expect(diffRooms([a, b], [a, b])).toEqual([]);
		// Same reference but mutated in place: by contract the reference wins.
		a.name = "mutated";
		expect(diffRooms([a, b], [a, b])).toEqual([]);
	});

	it("treats undefined and a missing key as no change", () => {
		const next = room("r1", ["h1"], {
			cwd: undefined,
			branch: undefined,
		} as unknown as Partial<Room>);
		next.harnesses = [harness("h1", { sessionId: undefined } as unknown as Partial<Harness>)];
		expect(diffRooms([room("r1")], [next])).toEqual([]);
	});
});

describe("diffRooms: client-only harness keys", () => {
	const withGen = (n: number, name = "h1"): Room => {
		const r = room("r1");
		r.harnesses = [harness("h1", { name, spawnGen: n } as unknown as Partial<Harness>)];
		return r;
	};

	it("a spawnGen bump alone emits nothing", () => {
		expect(diffRooms([withGen(0)], [withGen(1)])).toEqual([]);
	});

	it("a spawnGen bump with a real harness change emits one putHarness", () => {
		const next = withGen(1, "renamed");
		expect(diffRooms([withGen(0)], [next])).toEqual([
			{ type: "putHarness", roomId: "r1", harness: next.harnesses[0] },
		]);
	});
});

describe("diffRooms: launch mode (#752)", () => {
	const withLaunch = (launch?: Harness["launch"]): Room => {
		const r = room("r1");
		r.harnesses = [harness("h1", launch ? { launch } : {})];
		return r;
	};
	const shell: Harness["launch"] = { mode: "shell", argv: ["bash"] };

	it("setting launch emits putHarness, and a remote put applies it", () => {
		const next = withLaunch(shell);
		const out = diffRooms([withLaunch()], [next]);
		expect(out).toEqual([{ type: "putHarness", roomId: "r1", harness: next.harnesses[0] }]);
		const applied = applyRoomChanges([withLaunch()], [{ type: "roomPut", room: next }]);
		expect(applied[0]?.harnesses[0]?.launch).toEqual(shell);
	});

	it("removing launch emits putHarness, and a remote put clears it", () => {
		const next = withLaunch();
		const out = diffRooms([withLaunch(shell)], [next]);
		expect(out).toEqual([{ type: "putHarness", roomId: "r1", harness: next.harnesses[0] }]);
		const applied = applyRoomChanges([withLaunch(shell)], [{ type: "roomPut", room: next }]);
		expect(applied[0]?.harnesses[0]?.launch).toBeUndefined();
	});

	it("round trips through the reference store both ways", () => {
		for (const [a, b] of [
			[withLaunch(), withLaunch(shell)],
			[withLaunch(shell), withLaunch()],
		] as const) {
			expect(applyMutations(clone([a]), diffRooms([a], [b])).rooms).toEqual([b]);
		}
	});
});

describe("round trip through the reference store", () => {
	const cases: [string, Room[], Room[]][] = [
		["create", [room("a")], [room("a"), room("b", ["x", "y"])]],
		["remove", [room("a"), room("b")], [room("b")]],
		["room edit", [room("a")], [room("a", ["h1"], { name: "z", cwd: "/tmp/z" })]],
		["room field cleared", [room("a", ["h1"], { cwd: "/c" })], [room("a")]],
		["harness add", [room("a")], [room("a", ["h1", "h2"])]],
		["harness remove", [room("a", ["h1", "h2"])], [room("a", ["h2"])]],
		["harness reorder", [room("a", ["x", "y", "z"])], [room("a", ["z", "x", "y"])]],
		["room reorder", [room("a"), room("b"), room("c")], [room("c"), room("a"), room("b")]],
		[
			"everything at once",
			[room("a", ["x", "y"]), room("b"), room("c")],
			[room("d", ["q"]), room("c", ["h1", "n"], { name: "cc" }), room("a", ["y"], { badge: 2 })],
		],
		["new room inserted mid-list", [room("a"), room("b")], [room("a"), room("n"), room("b")]],
	];
	for (const [name, prev, next] of cases) {
		it(name, () => {
			const { rooms } = applyMutations(clone(prev), diffRooms(prev, next));
			expect(rooms).toEqual(next);
		});
	}
});

describe("two-client property (#729 acceptance)", () => {
	it("two clients adding different rooms converge on the daemon's state", () => {
		const baseline = [room("r1")];
		const a = [room("r1"), room("r2")];
		const b = [room("r1"), room("r3")];

		const batchA = applyMutations(clone(baseline), diffRooms(baseline, a));
		const batchB = applyMutations(batchA.rooms, diffRooms(baseline, b));
		const model = batchB.rooms;
		expect(model.map((r) => r.id).sort()).toEqual(["r1", "r2", "r3"]);

		const aFinal = applyRoomChanges(a, batchB.changes);
		const bFinal = applyRoomChanges(applyRoomChanges(b, batchA.changes), []);
		expect(aFinal).toEqual(model);
		expect(bFinal).toEqual(model);
	});

	it("a harness edit and a rename of the same room both survive in the store", () => {
		const baseline = [room("r1", ["h1"])];
		const a = clone(baseline);
		const a0 = a[0] as Room;
		a0.harnesses = [harness("h1", { name: "edited" })];
		const b = clone(baseline);
		(b[0] as Room).name = "renamed";

		const batchA = applyMutations(clone(baseline), diffRooms(baseline, a));
		const batchB = applyMutations(batchA.rooms, diffRooms(baseline, b));
		const model = batchB.rooms;
		expect(model).toHaveLength(1);
		expect((model[0] as Room).name).toBe("renamed");
		expect((model[0] as Room).harnesses[0]?.name).toBe("edited");

		// A receives B's batch (which carries the merged room) and converges.
		expect(applyRoomChanges(a, batchB.changes)).toEqual(model);
		// B receives A's batch, then the store's later state for the room.
		const bAfterA = applyRoomChanges(b, batchA.changes);
		expect(applyRoomChanges(bAfterA, batchB.changes)).toEqual(model);
	});
});
