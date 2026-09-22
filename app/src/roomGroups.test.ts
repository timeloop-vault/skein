import { describe, expect, it } from "vitest";
import {
	allRoomOrder,
	buildStrip,
	groupKey,
	groupRooms,
	resolveRowDrop,
	resolveTopDrop,
	roomIsGroupMain,
	segmentId,
	segmentOfRoom,
	topLevelTarget,
} from "./roomGroups.ts";
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

describe("groupKey", () => {
	it("is null for a non-git room", () => {
		expect(groupKey(room("a"))).toBeNull();
	});

	it("normalizes backslashes, trailing slash and case", () => {
		const mixed = groupKey(room("a", { repoRoot: "C:\\Repo\\Proj\\" }));
		const plain = groupKey(room("b", { repoRoot: "c:/repo/proj" }));
		expect(mixed).toBe(plain);
		expect(mixed).toBe("c:/repo/proj");
	});
});

describe("roomIsGroupMain", () => {
	it("is true when the room's own cwd normalizes to the key", () => {
		const key = groupKey(room("main", { repoRoot: "C:/Repo/Proj" }));
		expect(key).not.toBeNull();
		const r = room("main", { repoRoot: "C:/Repo/Proj", cwd: "c:\\repo\\proj\\" });
		expect(roomIsGroupMain(r, key ?? "")).toBe(true);
	});

	it("is false for a worktree room whose cwd differs from the key", () => {
		const key = groupKey(room("wt", { repoRoot: "C:/Repo/Proj" }));
		const r = room("wt", { repoRoot: "C:/Repo/Proj", cwd: "C:/Repo/Proj-wt/feature" });
		expect(roomIsGroupMain(r, key ?? "")).toBe(false);
	});

	it("is false for a room with no cwd", () => {
		expect(roomIsGroupMain(room("none"), "c:/repo/proj")).toBe(false);
	});
});

describe("buildStrip", () => {
	it("renders a lone main room as a plain tab", () => {
		const r = room("main", { repoRoot: "/repo", cwd: "/repo" });
		const segs = buildStrip([r]);
		expect(segs).toEqual([{ kind: "plain", room: r }]);
	});

	it("renders a non-git room as a plain tab", () => {
		const r = room("solo");
		const segs = buildStrip([r]);
		expect(segs).toEqual([{ kind: "plain", room: r }]);
	});

	it("groups a main with its worktrees, main first, even when main is last in the array", () => {
		const wt1 = room("wt1", { repoRoot: "/repo", cwd: "/repo-wt/a" });
		const wt2 = room("wt2", { repoRoot: "/repo", cwd: "/repo-wt/b" });
		const main = room("main", { repoRoot: "/repo", cwd: "/repo" });
		const segs = buildStrip([wt1, wt2, main]);
		expect(segs).toHaveLength(1);
		const seg = segs[0];
		if (seg?.kind !== "group") {
			throw new Error("expected a group segment");
		}
		expect(seg.lead?.id).toBe("main");
		expect(seg.members.map((m) => m.id)).toEqual(["wt1", "wt2"]);
		expect(seg.key).toBe("/repo");
	});

	it("renders a worktree-only bucket as a placeholder-led group", () => {
		const wt = room("wt", { repoRoot: "/repo", cwd: "/repo-wt/a" });
		const segs = buildStrip([wt]);
		expect(segs).toEqual([
			{
				kind: "group",
				key: "/repo",
				label: "repo",
				lead: null,
				members: [wt],
			},
		]);
	});

	it("keeps two interleaved repos as contiguous segments in first-appearance order", () => {
		const aMain = room("a-main", { repoRoot: "/a", cwd: "/a" });
		const bMain = room("b-main", { repoRoot: "/b", cwd: "/b" });
		const aWt = room("a-wt", { repoRoot: "/a", cwd: "/a-wt/x" });
		const bWt = room("b-wt", { repoRoot: "/b", cwd: "/b-wt/x" });
		// Array order: a-main, b-main, a-wt, b-wt — first appearance of
		// /a is index 0, first appearance of /b is index 1, so /a's
		// segment sorts before /b's even though a-wt appears after
		// b-main in the array.
		const segs = buildStrip([aMain, bMain, aWt, bWt]);
		expect(segs).toHaveLength(2);
		expect(segs[0]).toMatchObject({ kind: "group", key: "/a" });
		expect(segs[1]).toMatchObject({ kind: "group", key: "/b" });
		if (segs[0]?.kind === "group") {
			expect(segs[0].members.map((m) => m.id)).toEqual(["a-wt"]);
		}
		if (segs[1]?.kind === "group") {
			expect(segs[1].members.map((m) => m.id)).toEqual(["b-wt"]);
		}
	});

	it("marks a second same-cwd main as a following member, not a second lead", () => {
		const first = room("first", { repoRoot: "/repo", cwd: "/repo" });
		const second = room("second", { repoRoot: "/repo", cwd: "/repo" });
		const segs = buildStrip([first, second]);
		const seg = segs[0];
		if (seg?.kind !== "group") {
			throw new Error("expected a group segment");
		}
		expect(seg.lead?.id).toBe("first");
		expect(seg.members.map((m) => m.id)).toEqual(["second"]);
	});
});

describe("segmentId", () => {
	it("is g:<key> for a group and r:<roomId> for a plain tab", () => {
		const main = room("main", { repoRoot: "/repo", cwd: "/repo" });
		const wt = room("wt", { repoRoot: "/repo", cwd: "/repo-wt/a" });
		const solo = room("solo");
		const segs = buildStrip([main, wt, solo]);
		expect(segs.map(segmentId)).toEqual(["g:/repo", "r:solo"]);
	});
});

describe("groupRooms", () => {
	it("returns lead + members for a group, the single room for a plain tab", () => {
		const main = room("main", { repoRoot: "/repo", cwd: "/repo" });
		const wt = room("wt", { repoRoot: "/repo", cwd: "/repo-wt/a" });
		const segs = buildStrip([main, wt]);
		const seg = segs[0];
		if (!seg) {
			throw new Error("expected a segment");
		}
		expect(groupRooms(seg).map((r) => r.id)).toEqual(["main", "wt"]);

		const solo = room("solo");
		const plainSegs = buildStrip([solo]);
		const plainSeg = plainSegs[0];
		if (!plainSeg) {
			throw new Error("expected a segment");
		}
		expect(groupRooms(plainSeg).map((r) => r.id)).toEqual(["solo"]);
	});
});

describe("allRoomOrder", () => {
	it("walks every room, plain tabs and group leads+members in strip order", () => {
		const main = room("main", { repoRoot: "/repo", cwd: "/repo" });
		const wt = room("wt", { repoRoot: "/repo", cwd: "/repo-wt/a" });
		const solo = room("solo");
		const segs = buildStrip([solo, main, wt]);
		expect(allRoomOrder(segs).map((r) => r.id)).toEqual(["solo", "main", "wt"]);
	});

	it("includes a placeholder group's members even with no lead", () => {
		const placeholderWt = room("pwt", { repoRoot: "/other", cwd: "/other-wt/a" });
		const segs = buildStrip([placeholderWt]);
		expect(allRoomOrder(segs).map((r) => r.id)).toEqual(["pwt"]);
	});
});

describe("segmentOfRoom", () => {
	const main = room("main", { repoRoot: "/repo", cwd: "/repo" });
	const wt = room("wt", { repoRoot: "/repo", cwd: "/repo-wt/a" });
	const solo = room("solo");
	const segs = buildStrip([main, wt, solo]);

	it("finds a plain room's own segment", () => {
		expect(segmentOfRoom(segs, "solo")).toBe(segs[1]);
	});

	it("finds a group by its lead", () => {
		expect(segmentOfRoom(segs, "main")).toBe(segs[0]);
	});

	it("finds a group by a member", () => {
		expect(segmentOfRoom(segs, "wt")).toBe(segs[0]);
	});

	it("is undefined for an id in no segment", () => {
		expect(segmentOfRoom(segs, "ghost")).toBeUndefined();
	});
});

describe("topLevelTarget", () => {
	const main = room("main", { repoRoot: "/repo", cwd: "/repo" });
	const wtA = room("wtA", { repoRoot: "/repo", cwd: "/repo-wt/a" });
	const wtB = room("wtB", { repoRoot: "/repo", cwd: "/repo-wt/b" });
	const solo = room("solo");
	const rooms = [main, wtA, wtB, solo];
	const segs = buildStrip(rooms);
	const groupSeg = segs.find((s) => s.kind === "group");
	const plainSeg = segs.find((s) => s.kind === "plain");
	if (!groupSeg || !plainSeg) {
		throw new Error("expected one group and one plain segment");
	}

	it("a plain tab always targets its own room", () => {
		expect(topLevelTarget(plainSeg, new Map(), rooms)?.id).toBe("solo");
	});

	it("a group with a valid last-used member targets that member", () => {
		const lastUsed = new Map([["/repo", "wtB"]]);
		expect(topLevelTarget(groupSeg, lastUsed, rooms)?.id).toBe("wtB");
	});

	it("a group with a valid last-used lead targets the lead", () => {
		const lastUsed = new Map([["/repo", "main"]]);
		expect(topLevelTarget(groupSeg, lastUsed, rooms)?.id).toBe("main");
	});

	it("a stale last-used id (room left the group / closed) falls back to the lead", () => {
		const lastUsed = new Map([["/repo", "gone"]]);
		expect(topLevelTarget(groupSeg, lastUsed, rooms)?.id).toBe("main");
	});

	it("no last-used entry falls back to the lead", () => {
		expect(topLevelTarget(groupSeg, new Map(), rooms)?.id).toBe("main");
	});

	it("a placeholder group (no lead) falls back to its first member", () => {
		const wt = room("wt", { repoRoot: "/other", cwd: "/other-wt/a" });
		const otherRooms = [wt];
		const otherSegs = buildStrip(otherRooms);
		const seg = otherSegs[0];
		if (!seg) {
			throw new Error("expected a segment");
		}
		expect(topLevelTarget(seg, new Map(), otherRooms)?.id).toBe("wt");
	});

	it("a placeholder group with a valid last-used member targets that member", () => {
		const wtX = room("wtX", { repoRoot: "/other", cwd: "/other-wt/x" });
		const wtY = room("wtY", { repoRoot: "/other", cwd: "/other-wt/y" });
		const otherRooms = [wtX, wtY];
		const otherSegs = buildStrip(otherRooms);
		const seg = otherSegs[0];
		if (!seg) {
			throw new Error("expected a segment");
		}
		const lastUsed = new Map([["/other", "wtY"]]);
		expect(topLevelTarget(seg, lastUsed, otherRooms)?.id).toBe("wtY");
	});
});

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
