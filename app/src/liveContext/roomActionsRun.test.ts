import { describe, expect, it, vi } from "vitest";

vi.mock("../perfTelemetry.ts", () => ({
	span: <T>(_name: string, fn: () => T): T => fn(),
}));

import { type RoomActionsIo, type RoomActionsRefs, startRoomActionsRun } from "./roomActionsRun.ts";
import { emptyRows, type RowsState } from "./roomRows.ts";
import type { HarnessAction } from "./store.ts";

const ROOM = "r1";
const act = (id: number): HarnessAction => ({
	id,
	harnessId: "h",
	roomId: ROOM,
	timestampMs: id,
	kind: "tool_call",
	payload: "{}",
	source: null,
});

function makeRefs(): RoomActionsRefs {
	return {
		stateRef: { current: emptyRows() },
		pendingRef: { current: [] },
		visibleRef: { current: true },
		deriveRef: { current: false },
		flushRef: { current: () => {} },
		idsRef: { current: new Set() },
		liveIdsRef: { current: new Set() },
		lastLiveRef: { current: 0 },
	};
}

/// A run's IO with a controllable backfill and an event listener that stays
/// attached until the run's cleanup has been awaited (as unlisten is async).
function makeIo() {
	let handler: ((e: { payload: HarnessAction }) => void) | undefined;
	let resolveBackfill: (rows: HarnessAction[]) => void = () => {};
	const views: RowsState[] = [];
	const io: RoomActionsIo = {
		listen: (h) => {
			handler = h;
			// Unlisten resolves late: the handler stays reachable in the test.
			return Promise.resolve(() => {});
		},
		fetchBackfill: () =>
			new Promise<HarnessAction[]>((res) => {
				resolveBackfill = res;
			}),
		setView: (s) => views.push(s),
		setLoading: () => {},
	};
	return {
		io,
		emit: (a: HarnessAction) => handler?.({ payload: a }),
		backfill: async (rows: HarnessAction[]) => {
			resolveBackfill(rows);
			await new Promise((r) => setTimeout(r, 0));
		},
		lastIds: () => (views[views.length - 1] ?? emptyRows()).rows.map((r) => r.id),
	};
}

describe("startRoomActionsRun", () => {
	it("delivers backfill, dedupes a live duplicate, and lands a new live id", async () => {
		const refs = makeRefs();
		const run = makeIo();
		startRoomActionsRun(ROOM, refs, run.io);
		await run.backfill([act(1), act(2)]);
		expect(run.lastIds()).toEqual([1, 2]);
		run.emit(act(2));
		run.emit(act(3));
		expect(run.lastIds()).toEqual([1, 2, 3]);
		expect([...refs.liveIdsRef.current]).toEqual([3]);
	});

	it("StrictMode mount-cleanup-mount still delivers the backfill (#740)", async () => {
		const refs = makeRefs();
		const run1 = makeIo();
		const run2 = makeIo();
		const cleanup1 = startRoomActionsRun(ROOM, refs, run1.io);
		cleanup1();
		startRoomActionsRun(ROOM, refs, run2.io);
		await run1.backfill([act(1), act(2), act(3)]);
		await run2.backfill([act(1), act(2), act(3)]);
		expect(run2.lastIds()).toEqual([1, 2, 3]);
	});

	it("a live event on a cleaned-up run's listener does not starve the new run (#740)", async () => {
		const refs = makeRefs();
		const run1 = makeIo();
		const run2 = makeIo();
		const cleanup1 = startRoomActionsRun(ROOM, refs, run1.io);
		cleanup1();
		startRoomActionsRun(ROOM, refs, run2.io);
		run1.emit(act(7));
		await run2.backfill([act(7)]);
		expect(run2.lastIds()).toEqual([7]);
	});
});
