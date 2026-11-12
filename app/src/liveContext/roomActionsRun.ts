// One run of the `useRoomActions` effect for a concrete room (#740 test seam).
//
// Extracted from store.ts so the listen/backfill/dedupe logic runs in node
// with injected IO. Each run owns its seen-id sets and ignores callbacks
// once cancelled, so a stale run (StrictMode's first mount) can't mark the
// next run's backfill as already seen (#740).

import { span } from "../perfTelemetry.ts";
import { applyRows, emptyRows, type RowsState } from "./roomRows.ts";
import type { HarnessAction } from "./store.ts";

interface Ref<T> {
	current: T;
}

/// The refs `useRoomActions` shares across effect runs.
export interface RoomActionsRefs {
	stateRef: Ref<RowsState>;
	pendingRef: Ref<HarnessAction[]>;
	visibleRef: Ref<boolean>;
	deriveRef: Ref<boolean>;
	flushRef: Ref<(reveal?: boolean) => void>;
	idsRef: Ref<Set<number>>;
	liveIdsRef: Ref<Set<number>>;
	lastLiveRef: Ref<number>;
}

export interface RoomActionsIo {
	listen: (handler: (event: { payload: HarnessAction }) => void) => Promise<() => void>;
	fetchBackfill: (roomId: string) => Promise<HarnessAction[]>;
	setView: (state: RowsState) => void;
	setLoading: (loading: boolean) => void;
}

/// Starts a run; the returned function is the effect cleanup.
export function startRoomActionsRun(
	roomId: string,
	refs: RoomActionsRefs,
	io: RoomActionsIo,
): () => void {
	const { stateRef, pendingRef, visibleRef, deriveRef, flushRef, idsRef, liveIdsRef, lastLiveRef } =
		refs;
	let cancelled = false;
	stateRef.current = emptyRows();
	pendingRef.current = [];
	io.setView(stateRef.current);
	// Per-run sets (#740): under StrictMode a cleaned-up run's late callbacks
	// must not touch the set the next run is deduping against.
	const ids = new Set<number>();
	const liveIds = new Set<number>();
	idsRef.current = ids;
	liveIdsRef.current = liveIds;
	lastLiveRef.current = 0;
	io.setLoading(true);

	const flush = (reveal = false) => {
		if (cancelled) return;
		if (pendingRef.current.length === 0 && !reveal) return;
		// Only flushes that do work are timed (the guards above are no-ops).
		span("liveContext.flush", () => {
			const batch = pendingRef.current;
			pendingRef.current = [];
			const res = applyRows(stateRef.current, batch, { reveal, derive: deriveRef.current });
			if (res.state === stateRef.current) return;
			for (const t of res.trimmed) liveIds.delete(t.id);
			stateRef.current = res.state;
			io.setView(res.state);
		});
	};
	flushRef.current = flush;

	const enqueue = (rows: HarnessAction[]) => {
		if (cancelled) return;
		pendingRef.current.push(...rows);
		if (visibleRef.current) flush();
	};

	// Attach the listener BEFORE the backfill query so a row
	// inserted in the gap still lands (de-duped by id if the query
	// also returned it).
	const unlistenPromise = io.listen((event) =>
		span("liveContext.apply", () => {
			if (cancelled) return;
			const a = event.payload;
			if (a.roomId !== roomId || ids.has(a.id)) return;
			ids.add(a.id);
			liveIds.add(a.id);
			lastLiveRef.current = Date.now();
			enqueue([a]);
		}),
	);

	void io
		.fetchBackfill(roomId)
		.then((rows) => {
			if (cancelled) return;
			const fresh = rows.filter((a) => !ids.has(a.id));
			for (const a of fresh) ids.add(a.id);
			enqueue(fresh);
		})
		.catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn(`[skein] db_recent_harness_actions_by_room failed for ${roomId}:`, msg);
		})
		.finally(() => {
			if (!cancelled) io.setLoading(false);
		});

	return () => {
		cancelled = true;
		void unlistenPromise.then((un) => un());
	};
}
