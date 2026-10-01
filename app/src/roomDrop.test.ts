import { describe, expect, it } from "vitest";
import { resolveRowDrop, resolveTopDrop } from "./roomDrop.ts";
import type { Room } from "./types.ts";

function room(id: string, opts: Partial<Room> = {}): Room {
	return {
		id,
		name: id,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: [],
		activeHarnessId: "",
		...opts,
	};
}

describe("resolveRowDrop", () => {
	const main = room("main", { repoRoot: "/repo", cwd: "/repo" });
	const wtA = room("wtA", { repoRoot: "/repo", cwd: "/repo-wt/a" });
	const wtB = room("wtB", { repoRoot: "/repo", cwd: "/repo-wt/b" });
	const wtC = room("wtC", { repoRoot: "/repo", cwd: "/repo-wt/c" });
	const otherMain = room("otherMain", { repoRoot: "/other", cwd: "/other" });
	const otherWt = room("otherWt", { repoRoot: "/other", cwd: "/other-wt/a" });

	it("reorders two members within the same group, before", () => {
		const rooms = [main, wtA, wtB, otherMain, otherWt];
		const moved = resolveRowDrop(rooms, "wtB", "wtA", "before");
		expect(moved.map((r) => r.id)).toEqual(["main", "wtB", "wtA", "otherMain", "otherWt"]);
	});

	it("reorders two members within the same group, after", () => {
		const rooms = [main, wtA, wtB, otherMain, otherWt];
		const moved = resolveRowDrop(rooms, "wtB", "wtA", "after");
		expect(moved.map((r) => r.id)).toEqual(["main", "wtA", "wtB", "otherMain", "otherWt"]);
	});

	it("moves a member into the group's last slot via after", () => {
		const rooms = [main, wtA, wtB, wtC];
		const moved = resolveRowDrop(rooms, "wtA", "wtC", "after");
		expect(moved.map((r) => r.id)).toEqual(["main", "wtB", "wtC", "wtA"]);
	});

	it("is a no-op across groups", () => {
		const rooms = [main, wtA, wtB, otherMain, otherWt];
		const moved = resolveRowDrop(rooms, "wtA", "otherWt", "before");
		expect(moved).toBe(rooms);
	});

	it("is a no-op when either id is a lead (the lead is pinned, not draggable)", () => {
		const rooms = [main, wtA, wtB];
		expect(resolveRowDrop(rooms, "main", "wtA", "before")).toBe(rooms);
		expect(resolveRowDrop(rooms, "wtA", "main", "before")).toBe(rooms);
	});

	it("is a no-op dragging a room onto itself", () => {
		const rooms = [main, wtA, wtB];
		expect(resolveRowDrop(rooms, "wtA", "wtA", "before")).toBe(rooms);
	});
});

describe("resolveTopDrop", () => {
	const aMain = room("a-main", { repoRoot: "/a", cwd: "/a" });
	const aWt = room("a-wt", { repoRoot: "/a", cwd: "/a-wt/x" });
	const bMain = room("b-main", { repoRoot: "/b", cwd: "/b" });
	const solo = room("solo");

	it("moves a whole group before another segment, staying contiguous", () => {
		// b-main is a lone main room (plain tab, segment id "r:b-main");
		// the /a bucket has a worktree too, so it's the group here.
		const rooms = [bMain, aMain, aWt, solo];
		const moved = resolveTopDrop(rooms, "g:/a", "r:b-main", "before");
		expect(moved.map((r) => r.id)).toEqual(["a-main", "a-wt", "b-main", "solo"]);
	});

	it("moves a plain room after a group segment", () => {
		const rooms = [solo, aMain, aWt, bMain];
		const moved = resolveTopDrop(rooms, "r:solo", "g:/a", "after");
		expect(moved.map((r) => r.id)).toEqual(["a-main", "a-wt", "solo", "b-main"]);
	});

	it("is a no-op when a segment id doesn't resolve", () => {
		const rooms = [aMain, aWt];
		expect(resolveTopDrop(rooms, "r:missing", "g:/a", "before")).toBe(rooms);
	});

	it("is a no-op dropping a segment onto a second-row room id (not a segment id)", () => {
		const rooms = [aMain, aWt, bMain];
		// "a-wt" is a member's raw room id, never a valid segmentId
		// ("g:/a" is a-wt's segment) — so this can't resolve as a
		// top-row drop target at all.
		expect(resolveTopDrop(rooms, "r:b-main", "a-wt", "before")).toBe(rooms);
	});

	it("is a no-op dropping a segment onto itself", () => {
		const rooms = [aMain, aWt, bMain];
		expect(resolveTopDrop(rooms, "g:/a", "g:/a", "before")).toBe(rooms);
	});
});
