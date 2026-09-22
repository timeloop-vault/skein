import { describe, expect, it } from "vitest";
import {
	DRAG_THRESHOLD_PX,
	type TabDragHit,
	type TabDragState,
	cancel,
	gapForX,
	gapToTarget,
	idleState,
	move,
	press,
	release,
	sideFor,
} from "./tabDrag.ts";

// Pure state-machine tests — no DOM, no React. `move`'s `hit` is built by
// hand to stand in for whatever `elementFromPoint` + `closest()` would
// have found in the browser. `order` stands in for the same-kind sibling
// ids `useTabDrag` reads off the DOM at press — the ids of the whole
// strip (same room, for a harness drag), left to right.

const roomHit = (id: string, side: "before" | "after"): TabDragHit => ({ kind: "room", id, side });
const harnessHit = (roomId: string, id: string, side: "before" | "after"): TabDragHit => ({
	kind: "harness",
	roomId,
	id,
	side,
});

describe("tabDrag", () => {
	it("does not become a drag below the threshold — release yields no reorder", () => {
		let state = press({ kind: "room", id: "r1" }, 1, 100, 100, ["r1", "r2"]);
		state = move(state, 1, 100 + DRAG_THRESHOLD_PX - 1, 100, roomHit("r2", "after"));
		expect(state.status).toBe("pressed");
		const { state: next, reorder } = release(state);
		expect(next).toEqual(idleState);
		expect(reorder).toBeUndefined();
	});

	it("crosses the threshold and reorders a room before the target", () => {
		// r1 is already immediately before r2 — "before r2" would be a
		// no-op (see the dedicated no-op tests below) — so this uses a
		// non-adjacent target (r3) to exercise a real move.
		let state = press({ kind: "room", id: "r1" }, 1, 100, 100, ["r1", "r2", "r3"]);
		state = move(state, 1, 100 + DRAG_THRESHOLD_PX, 100, roomHit("r3", "before"));
		expect(state.status).toBe("dragging");
		const { state: next, reorder } = release(state);
		expect(next).toEqual(idleState);
		expect(reorder).toEqual({ info: { kind: "room", id: "r1" }, targetId: "r3", side: "before" });
	});

	it("reorders a room after the target", () => {
		let state = press({ kind: "room", id: "r1" }, 1, 0, 0, ["r1", "r2", "r3"]);
		state = move(state, 1, 50, 0, roomHit("r3", "after"));
		const { reorder } = release(state);
		expect(reorder).toEqual({ info: { kind: "room", id: "r1" }, targetId: "r3", side: "after" });
	});

	it("reorders a harness within the same room", () => {
		// Same non-adjacency note as the room case above — h1 is already
		// immediately before h2, so this targets h3 instead.
		let state = press({ kind: "harness", roomId: "r1", id: "h1" }, 1, 0, 0, ["h1", "h2", "h3"]);
		state = move(state, 1, 50, 0, harnessHit("r1", "h3", "before"));
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") expect(state.refused).toBe(false);
		const { reorder } = release(state);
		expect(reorder).toEqual({
			info: { kind: "harness", roomId: "r1", id: "h1" },
			targetId: "h3",
			side: "before",
		});
	});

	it("refuses a cross-room harness target and yields no reorder on release", () => {
		let state = press({ kind: "harness", roomId: "r1", id: "h1" }, 1, 0, 0, ["h1", "h2"]);
		state = move(state, 1, 50, 0, harnessHit("r2", "h9", "after"));
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") {
			expect(state.refused).toBe(true);
			expect(state.target).toBeNull();
		}
		const { state: next, reorder } = release(state);
		expect(next).toEqual(idleState);
		expect(reorder).toBeUndefined();
	});

	it("ignores a cross-kind hit (room drag over a harness tab) without refusing", () => {
		let state = press({ kind: "room", id: "r1" }, 1, 0, 0, ["r1"]);
		state = move(state, 1, 50, 0, harnessHit("r1", "h1", "after"));
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") {
			expect(state.target).toBeNull();
			expect(state.refused).toBe(false);
		}
	});

	it("ignores a cross-kind hit (harness drag over a room tab) without refusing", () => {
		let state = press({ kind: "harness", roomId: "r1", id: "h1" }, 1, 0, 0, ["h1"]);
		state = move(state, 1, 50, 0, roomHit("r1", "before"));
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") {
			expect(state.target).toBeNull();
			expect(state.refused).toBe(false);
		}
	});

	it("dropping a tab on itself is a no-op", () => {
		let state = press({ kind: "room", id: "r1" }, 1, 0, 0, ["r1", "r2"]);
		state = move(state, 1, 50, 0, roomHit("r1", "after"));
		const { state: next, reorder } = release(state);
		expect(next).toEqual(idleState);
		expect(reorder).toBeUndefined();
	});

	it("a null hit mid-drag clears the target without refusing", () => {
		let state = press({ kind: "room", id: "r1" }, 1, 0, 0, ["r1", "r2"]);
		state = move(state, 1, 50, 0, roomHit("r2", "after"));
		state = move(state, 1, 60, 0, null);
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") {
			expect(state.target).toBeNull();
			expect(state.refused).toBe(false);
		}
	});

	it("cancel resets to idle from any state", () => {
		expect(cancel()).toEqual(idleState);
	});

	it("ignores a move for a pointerId other than the one pressed", () => {
		const pressed = press({ kind: "room", id: "r1" }, 1, 0, 0, ["r1", "r2"]);
		const after = move(pressed, 2, 999, 999, roomHit("r2", "after"));
		expect(after).toBe(pressed);
	});

	it("release on an untouched idle state is a no-op", () => {
		const { state, reorder } = release(idleState as TabDragState);
		expect(state).toEqual(idleState);
		expect(reorder).toBeUndefined();
	});
});

// #271: a target is a (id, side) pair, but two different (id, side)
// pairs can name the SAME gap — "after r1" and "before r2" are the same
// slot in ["r1", "r2", "r3"]. Both used to light up even though neither
// one moves anything, and the first real move was only past r2's own
// midpoint. These tests hover the leftmost tab over every position that
// maps to its own current spot and check none of them produce a target.
describe("tabDrag — no-op gaps (#271)", () => {
	const order = ["r1", "r2", "r3"];

	it("own right half is a no-op", () => {
		let state = press({ kind: "room", id: "r1" }, 1, 0, 0, order);
		state = move(state, 1, 50, 0, roomHit("r1", "after"));
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") expect(state.target).toBeNull();
	});

	it("the right neighbour's left half is the same no-op gap", () => {
		let state = press({ kind: "room", id: "r1" }, 1, 0, 0, order);
		state = move(state, 1, 50, 0, roomHit("r2", "before"));
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") expect(state.target).toBeNull();
	});

	it("the first real move is past the right neighbour's own midpoint", () => {
		let state = press({ kind: "room", id: "r1" }, 1, 0, 0, order);
		state = move(state, 1, 50, 0, roomHit("r2", "after"));
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") expect(state.target).toEqual({ id: "r2", side: "after" });
		const { reorder } = release(state);
		expect(reorder).toEqual({ info: { kind: "room", id: "r1" }, targetId: "r2", side: "after" });
	});

	it("a middle tab's own two bounding gaps are both no-ops", () => {
		// r2 sits at gap 1..2; "before r2" (gap 1) and "after r2" (gap 2)
		// are both where it already is.
		let state = press({ kind: "room", id: "r2" }, 1, 0, 0, order);
		state = move(state, 1, 50, 0, roomHit("r2", "before"));
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") expect(state.target).toBeNull();
		state = move(state, 1, 50, 0, roomHit("r2", "after"));
		if (state.status === "dragging") expect(state.target).toBeNull();
		state = move(state, 1, 50, 0, roomHit("r1", "after"));
		if (state.status === "dragging") expect(state.target).toBeNull();
		state = move(state, 1, 50, 0, roomHit("r3", "before"));
		if (state.status === "dragging") expect(state.target).toBeNull();
	});

	it("a same-room harness no-op suppresses the indicator too", () => {
		let state = press({ kind: "harness", roomId: "r1", id: "h1" }, 1, 0, 0, ["h1", "h2"]);
		state = move(state, 1, 50, 0, harnessHit("r1", "h1", "after"));
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") {
			expect(state.target).toBeNull();
			expect(state.refused).toBe(false);
		}
	});
});

// #76: group-aware room drag validity. RoomStrip mints `segId`/`role`
// off `data-drag-seg`/`data-drag-role` and never leaves them out for a
// group-relevant tab, but the defaulting (missing → `"segment"` / the
// id itself) is what keeps every plain-room test above compiling and
// passing unchanged.
describe("tabDrag — group-aware room validity (#76)", () => {
	it("a member refuses a target in a different group and shows no indicator", () => {
		let state = press({ kind: "room", id: "wtA", segId: "g:/a", role: "member" }, 1, 0, 0, [
			"main",
			"wtA",
			"otherWt",
		]);
		state = move(state, 1, 50, 0, {
			kind: "room",
			id: "otherWt",
			side: "after",
			segId: "g:/other",
			role: "member",
		});
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") {
			expect(state.refused).toBe(true);
			expect(state.target).toBeNull();
		}
		const { state: next, reorder } = release(state);
		expect(next).toEqual(idleState);
		expect(reorder).toBeUndefined();
	});

	it("a member allows a target in its own group", () => {
		let state = press({ kind: "room", id: "wtA", segId: "g:/a", role: "member" }, 1, 0, 0, [
			"main",
			"wtA",
			"wtB",
		]);
		state = move(state, 1, 50, 0, {
			kind: "room",
			id: "wtB",
			side: "after",
			segId: "g:/a",
			role: "member",
		});
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") {
			expect(state.refused).toBe(false);
			expect(state.target).toEqual({ id: "wtB", side: "after" });
		}
	});

	it("a segment (lead) refuses a member of its OWN group — dropping inside itself", () => {
		let state = press({ kind: "room", id: "main", segId: "g:/a", role: "segment" }, 1, 0, 0, [
			"main",
			"wtA",
			"wtB",
		]);
		state = move(state, 1, 50, 0, {
			kind: "room",
			id: "wtA",
			side: "before",
			segId: "g:/a",
			role: "member",
		});
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") {
			expect(state.refused).toBe(true);
			expect(state.target).toBeNull();
		}
	});

	it("a segment allows a member of a DIFFERENT group", () => {
		let state = press({ kind: "room", id: "main", segId: "g:/a", role: "segment" }, 1, 0, 0, [
			"main",
			"otherWt",
		]);
		state = move(state, 1, 50, 0, {
			kind: "room",
			id: "otherWt",
			side: "after",
			segId: "g:/other",
			role: "member",
		});
		expect(state.status).toBe("dragging");
		if (state.status === "dragging") {
			expect(state.refused).toBe(false);
			expect(state.target).toEqual({ id: "otherWt", side: "after" });
		}
	});
});

describe("gapToTarget", () => {
	const order = ["a", "b", "c", "d"];

	it("gap 0 lands before the first tab", () => {
		expect(gapToTarget(order, 0)).toEqual({ id: "a", side: "before" });
	});

	it("gap n lands after the last tab", () => {
		expect(gapToTarget(order, order.length)).toEqual({ id: "d", side: "after" });
	});

	it("a middle gap lands before the tab at that index", () => {
		expect(gapToTarget(order, 2)).toEqual({ id: "c", side: "before" });
	});

	it("rejects an out-of-range gap", () => {
		expect(gapToTarget(order, -1)).toBeNull();
		expect(gapToTarget(order, order.length + 1)).toBeNull();
	});

	it("rejects an empty strip", () => {
		expect(gapToTarget([], 0)).toBeNull();
	});
});

// #271: reorderRoom / reorderHarness (App.tsx) do the actual array
// splice; this is a byte-for-byte local copy of their arithmetic so the
// gap→(targetId, side) translation can be checked against the order the
// user actually ends up seeing, without importing the React component
// tree into a unit test.
function applySplice(
	order: string[],
	fromId: string,
	targetId: string,
	side: "before" | "after",
): string[] {
	const fromIdx = order.indexOf(fromId);
	const targetIdx = order.indexOf(targetId);
	if (fromIdx < 0 || targetIdx < 0 || fromId === targetId) return order;
	const adjustedTarget = side === "after" ? targetIdx + 1 : targetIdx;
	const insertIdx = fromIdx < adjustedTarget ? adjustedTarget - 1 : adjustedTarget;
	if (fromIdx === insertIdx) return order;
	const next = [...order];
	const [item] = next.splice(fromIdx, 1);
	if (!item) return order;
	next.splice(insertIdx, 0, item);
	return next;
}

describe("gap → splice: every non-no-op gap lands where the indicator said", () => {
	const order = ["a", "b", "c", "d"];

	it("moving left: dragging 'c' to gap 0 puts it first", () => {
		const target = gapToTarget(order, 0);
		expect(target).not.toBeNull();
		if (!target) return;
		expect(applySplice(order, "c", target.id, target.side)).toEqual(["c", "a", "b", "d"]);
	});

	it("moving left: dragging 'd' to gap 1 puts it right after 'a'", () => {
		const target = gapToTarget(order, 1);
		expect(target).not.toBeNull();
		if (!target) return;
		expect(applySplice(order, "d", target.id, target.side)).toEqual(["a", "d", "b", "c"]);
	});

	it("moving right: dragging 'b' to gap 3 puts it right before 'd'", () => {
		const target = gapToTarget(order, 3);
		expect(target).not.toBeNull();
		if (!target) return;
		expect(applySplice(order, "b", target.id, target.side)).toEqual(["a", "c", "b", "d"]);
	});

	it("to gap n: dragging 'b' to the end puts it last", () => {
		const target = gapToTarget(order, order.length);
		expect(target).not.toBeNull();
		if (!target) return;
		expect(applySplice(order, "b", target.id, target.side)).toEqual(["a", "c", "d", "b"]);
	});

	it("every gap that isn't the source's own position actually moves it", () => {
		for (const sourceId of order) {
			const s = order.indexOf(sourceId);
			for (let gap = 0; gap <= order.length; gap++) {
				const target = gapToTarget(order, gap);
				expect(target).not.toBeNull();
				if (!target) continue;
				const result = applySplice(order, sourceId, target.id, target.side);
				if (gap === s || gap === s + 1) {
					// The gaps tabDrag.ts calls no-ops (and never even builds a
					// target for) really are no-ops against the real splice too.
					expect(result).toEqual(order);
				} else {
					expect(result).not.toEqual(order);
					expect(result.indexOf(sourceId)).toBe(gap < s ? gap : gap - 1);
				}
			}
		}
	});
});

describe("sideFor", () => {
	it("picks before when x is left of the midpoint", () => {
		expect(sideFor(10, 0, 100)).toBe("before");
	});

	it("picks after when x is right of the midpoint", () => {
		expect(sideFor(60, 0, 100)).toBe("after");
	});

	it("picks after exactly at the midpoint", () => {
		expect(sideFor(50, 0, 100)).toBe("after");
	});
});

// #271: gapForX is the one rule for "pointer isn't directly on a tab" —
// blank strip space, the +harness/+room button, and the sub-pixel seam
// between two adjacent tabs all go through it. Three 100px-wide tabs,
// back to back: midpoints at 50, 150, 250.
describe("gapForX", () => {
	const rects = [
		{ left: 0, width: 100 },
		{ left: 100, width: 100 },
		{ left: 200, width: 100 },
	];

	it("before the first tab's midpoint is gap 0", () => {
		expect(gapForX(-10, rects)).toBe(0);
		expect(gapForX(0, rects)).toBe(0);
	});

	it("after the last tab's midpoint is gap n", () => {
		expect(gapForX(260, rects)).toBe(3);
		expect(gapForX(1000, rects)).toBe(3);
	});

	it("between two midpoints, both sides of the gap agree", () => {
		// Anywhere strictly between tab 0's midpoint (50) and tab 1's
		// midpoint (150) is gap 1, whether it's the sliver right after 50
		// or the sliver right before 150.
		expect(gapForX(51, rects)).toBe(1);
		expect(gapForX(99, rects)).toBe(1);
		expect(gapForX(101, rects)).toBe(1);
		expect(gapForX(149, rects)).toBe(1);
	});

	it("exactly on a midpoint counts that tab — consistent with sideFor's x===mid tie", () => {
		// sideFor(mid, ...) picks "after" (x < mid is the only "before"
		// case), which for a hit on tab i means gap i+1. gapForX must land
		// on the same gap when fed the same x directly, so the tie here
		// also resolves to "count this tab", i.e. i+1.
		expect(gapForX(50, rects)).toBe(1);
		expect(gapForX(150, rects)).toBe(2);
		expect(gapForX(250, rects)).toBe(3);
	});

	it("empty rects is gap 0", () => {
		expect(gapForX(500, [])).toBe(0);
	});
});
