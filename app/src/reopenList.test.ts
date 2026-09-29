import { describe, expect, it } from "vitest";
import {
	click,
	emptySelection,
	extendTo,
	matchesQuery,
	pruneTo,
	retireRooms,
	selectAll,
	toggle,
	unretireRooms,
	visibleArchived,
} from "./reopenList.ts";
import type { Room } from "./types.ts";

const room = (id: string, extra: Partial<Room> = {}): Room =>
	({ id, name: id, cwd: `/w/${id}`, harnesses: [], ...extra }) as unknown as Room;

const ids = ["a", "b", "c", "d", "e"];
const sorted = (s: ReadonlySet<string>) => [...s].sort();

describe("matchesQuery", () => {
	const r = room("x", { name: "Alpha", repo: "skein", branch: "feat/Zed", cwd: "/w/Path" });
	it.each([
		["", true],
		["  ", true],
		["alp", true],
		["SKEIN", true],
		["zed", true],
		["/w/path", true],
		["nope", false],
	])("%j -> %s", (q, want) => {
		expect(matchesQuery(r, q)).toBe(want);
	});
});

describe("visibleArchived", () => {
	const rooms = [
		room("open"),
		room("a1", { archived: 1 }),
		room("r1", { archived: 1, retired: 2 }),
		room("a2", { archived: 1 }),
	];
	it("hides open and retired rooms by default, keeping order", () => {
		expect(visibleArchived(rooms, "", false).map((r) => r.id)).toEqual(["a1", "a2"]);
	});
	it("shows both kinds with showRetired", () => {
		expect(visibleArchived(rooms, "", true).map((r) => r.id)).toEqual(["a1", "r1", "a2"]);
	});
	it("applies the query", () => {
		expect(visibleArchived(rooms, "r1", true).map((r) => r.id)).toEqual(["r1"]);
		expect(visibleArchived(rooms, "r1", false)).toEqual([]);
	});
});

describe("selection", () => {
	it("click selects one and anchors", () => {
		const s = click(click(emptySelection, "a"), "c");
		expect(sorted(s.ids)).toEqual(["c"]);
		expect(s.anchor).toBe("c");
	});
	it("toggle adds and removes, moving the anchor", () => {
		const s1 = toggle(click(emptySelection, "a"), "c");
		expect(sorted(s1.ids)).toEqual(["a", "c"]);
		const s2 = toggle(s1, "a");
		expect(sorted(s2.ids)).toEqual(["c"]);
		expect(s2.anchor).toBe("a");
	});
	it("extendTo selects the range, replacing the previous range", () => {
		const s1 = extendTo(click(emptySelection, "b"), ids, "d");
		expect(sorted(s1.ids)).toEqual(["b", "c", "d"]);
		const s2 = extendTo(s1, ids, "a");
		expect(sorted(s2.ids)).toEqual(["a", "b"]);
		expect(s2.anchor).toBe("b");
	});
	it("extendTo without an anchor acts as click", () => {
		const s = extendTo(emptySelection, ids, "c");
		expect(sorted(s.ids)).toEqual(["c"]);
		expect(s.anchor).toBe("c");
	});
	it("selectAll", () => {
		expect(sorted(selectAll(ids).ids)).toEqual(ids);
		expect(selectAll([])).toEqual({ ids: new Set(), anchor: null });
	});
	it("pruneTo drops hidden ids and a hidden anchor", () => {
		const s = pruneTo(extendTo(click(emptySelection, "a"), ids, "c"), ["b", "c", "d"]);
		expect(sorted(s.ids)).toEqual(["b", "c"]);
		expect(s.anchor).toBeNull();
	});
	it("pruneTo returns the same object when nothing changes", () => {
		const s = click(emptySelection, "a");
		expect(pruneTo(s, ids)).toBe(s);
	});
});

describe("retireRooms / unretireRooms", () => {
	const rooms = [room("open"), room("a", { archived: 1 }), room("b", { archived: 1 })];
	it("retires only archived rooms in ids", () => {
		const out = retireRooms(rooms, ["open", "a"], 99);
		expect(out[0]).not.toHaveProperty("retired");
		expect(out[1]?.retired).toBe(99);
		expect(out[2]).not.toHaveProperty("retired");
	});
	it("unretire removes the key entirely", () => {
		const out = unretireRooms(retireRooms(rooms, ["a", "b"], 5), ["a"]);
		expect(out[1]).not.toHaveProperty("retired");
		expect(out[1]?.archived).toBe(1);
		expect(out[2]?.retired).toBe(5);
	});
});
