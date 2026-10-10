import { describe, expect, it } from "vitest";
import { createRoomFold, deriveRoom, foldRows } from "./roomFold.ts";
import {
	applyRows,
	emptyRows,
	FEED_CAP,
	FEED_HARD_CAP,
	FEED_HARD_KEEP,
	FEED_KEEP,
	mergeById,
} from "./roomRows.ts";
import type { HarnessAction } from "./store.ts";

const mk = (id: number, kind = "tool_call", payload: unknown = {}): HarnessAction => ({
	id,
	harnessId: "h",
	roomId: "r",
	timestampMs: 1000 + id,
	kind,
	payload: JSON.stringify(payload),
	source: null,
});
const todo = (id: number) =>
	mk(id, "plan_change", {
		plan_item: { op: "create", id: `t${id}`, subject: `s${id}`, status: "pending" },
	});
const away = (id: number) => mk(id, "away_summary", { content: `away ${id}` });
const seq = (from: number, to: number) =>
	Array.from({ length: to - from + 1 }, (_, i) =>
		(from + i) % 3 === 0 ? todo(from + i) : mk(from + i),
	);
const fullFold = (rows: HarnessAction[]) => {
	const f = createRoomFold();
	foldRows(f, rows);
	return deriveRoom(f);
};

describe("applyRows", () => {
	it("folds in-order appends incrementally on the same fold", () => {
		const s0 = applyRows(emptyRows(), seq(1, 5)).state;
		const s1 = applyRows(s0, seq(6, 8)).state;
		expect(s1.fold).toBe(s0.fold);
		expect(s1.derived).toEqual(fullFold(seq(1, 8)));
		expect(s1.rows.map((r) => r.id)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
	});

	it("rebuilds on an out-of-order merge and matches a full fold", () => {
		const live = [away(10), todo(11)];
		const backfill = [away(2), todo(3), mk(4)];
		const s0 = applyRows(emptyRows(), live).state;
		const s1 = applyRows(s0, backfill).state;
		expect(s1.fold).not.toBe(s0.fold);
		expect(s1.rows.map((r) => r.id)).toEqual([2, 3, 4, 10, 11]);
		expect(s1.derived).toEqual(fullFold([...backfill, ...live]));
		expect(s1.derived.awaySummary?.content).toBe("away 10");
	});

	it("drops duplicates and returns the same state when nothing is new", () => {
		const s0 = applyRows(emptyRows(), seq(1, 4)).state;
		const r = applyRows(s0, [mk(2), mk(4)]);
		expect(r.state).toBe(s0);
		expect(r.state.derived.eventCount).toBe(4);
	});

	it("does not trim at FEED_CAP while visible; trims at FEED_HARD_CAP", () => {
		const all = seq(1, FEED_CAP + 1);
		const r = applyRows(emptyRows(), all);
		expect(r.state.rows).toHaveLength(FEED_CAP + 1);
		expect(r.trimmed).toHaveLength(0);
		const big = seq(1, FEED_HARD_CAP + 1);
		const r2 = applyRows(emptyRows(), big);
		expect(r2.state.rows).toHaveLength(FEED_HARD_KEEP);
		expect(r2.state.rows[0]?.id).toBe(FEED_HARD_CAP + 1 - FEED_HARD_KEEP + 1);
		expect(r2.state.derived).toEqual(fullFold(big));
	});

	it("a reveal flush trims an over-cap array even with nothing pending", () => {
		const s = applyRows(emptyRows(), seq(1, FEED_CAP + 50)).state;
		expect(s.rows).toHaveLength(FEED_CAP + 50);
		const r = applyRows(s, [], { reveal: true });
		expect(r.state.rows).toHaveLength(FEED_KEEP);
		expect(r.state.derived).toBe(s.derived);
		expect(applyRows(r.state, [], { reveal: true }).state).toBe(r.state);
	});

	it("trims to FEED_KEEP past FEED_CAP on reveal but keeps derived over every row", () => {
		const all = seq(1, FEED_CAP + 1);
		const r = applyRows(emptyRows(), all, { reveal: true });
		expect(r.state.rows).toHaveLength(FEED_KEEP);
		expect(r.state.rows[0]?.id).toBe(FEED_CAP + 1 - FEED_KEEP + 1);
		expect(r.trimmed).toHaveLength(FEED_CAP + 1 - FEED_KEEP);
		expect(r.state.derived).toEqual(fullFold(all));
		expect(r.state.derived.eventCount).toBe(FEED_CAP + 1);
		// Further appends keep folding on top of the trimmed state.
		const more = seq(FEED_CAP + 2, FEED_CAP + 10);
		const r2 = applyRows(r.state, more);
		expect(r2.state.derived).toEqual(fullFold([...all, ...more]));
	});

	it("reuses planGroups/totals/awaySummary identity when unchanged", () => {
		const s0 = applyRows(emptyRows(), [todo(3), away(4)]).state;
		const s1 = applyRows(s0, [mk(5), mk(6)]).state;
		expect(s1.derived).not.toBe(s0.derived);
		expect(s1.derived.planGroups).toBe(s0.derived.planGroups);
		expect(s1.derived.totals).toBe(s0.derived.totals);
		expect(s1.derived.awaySummary).toBe(s0.derived.awaySummary);
		const s2 = applyRows(s1, [todo(9)]).state;
		expect(s2.derived.planGroups).not.toBe(s1.derived.planGroups);
		expect(s2.derived).toEqual(fullFold([todo(3), away(4), mk(5), mk(6), todo(9)]));
	});

	it("derive:false merges rows but leaves the fold untouched", () => {
		const s0 = emptyRows();
		const r = applyRows(s0, seq(1, 6), { derive: false });
		expect(r.state.rows).toHaveLength(6);
		expect(r.state.fold).toBe(s0.fold);
		expect(r.state.derived).toBe(s0.derived);
		expect(r.state.derived.eventCount).toBe(0);
		// out-of-order merge too
		const r2 = applyRows(r.state, [mk(0)], { derive: false });
		expect(r2.state.rows.map((x) => x.id)).toEqual([0, 1, 2, 3, 4, 5, 6]);
		expect(r2.state.derived.eventCount).toBe(0);
	});

	it("append fast path and slow path agree with mergeById semantics", () => {
		const base = seq(10, 20);
		expect(mergeById(base, seq(21, 23)).map((r) => r.id)).toEqual([
			10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
		]);
		// unsorted incoming above the tail is sorted
		expect(
			mergeById(base, [mk(23), mk(21)])
				.map((r) => r.id)
				.slice(-2),
		).toEqual([21, 23]);
		// duplicates of low ids dropped, fresh high ids appended
		expect(
			mergeById(base, [mk(12), mk(21)])
				.map((r) => r.id)
				.slice(-2),
		).toEqual([20, 21]);
		expect(mergeById(base, [mk(12)])).toBe(base);
		// interleaved
		expect(mergeById(base, [mk(5), mk(15), mk(30)]).map((r) => r.id)).toEqual([
			5, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 30,
		]);
		expect(mergeById([], [mk(2), mk(1)]).map((r) => r.id)).toEqual([1, 2]);
	});

	it("buffered hidden rows flushed in one apply equal visible-all-along", () => {
		const rows = seq(1, 40);
		let visible = emptyRows();
		for (const r of rows) visible = applyRows(visible, [r]).state;
		const flushed = applyRows(emptyRows(), rows).state;
		expect(flushed.derived).toEqual(visible.derived);
		expect(flushed.rows).toEqual(visible.rows);
	});
});
