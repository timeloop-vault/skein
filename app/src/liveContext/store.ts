// Live Context store — issue #80.
//
// Loads the backfilled `harness_actions` history for a room (one query
// on mount) and tails live rows broadcast by the backend over the
// `harness-action` Tauri event. The three Live Context cards read from
// this.
//
// Ordering: rows are kept sorted ascending by `id` (the sqlite
// AUTOINCREMENT rowid, which is monotonic insertion order, i.e.
// chronological). The Activity card appends newest at the bottom and
// tails, so ascending-by-id is the natural store order.
//
// Race-safety: the live listener is attached *before* the backfill
// query runs, so any row inserted in the window between query and
// listen still lands. De-dupe is by `id` — a live event for a row the
// query already returned is dropped.
//
// Provenance: backfilled rows are never broadcast (see
// harness_action_event.rs) — the event channel only ever carries live
// rows. `liveIds` records the ids first seen on that channel, which is
// what the Activity card's backfill banner / slide-in distinguish on.
// An event for a row the query already delivered does NOT mark it live:
// the row was history by the time the user saw it.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { span } from "../perfTelemetry.ts";
import type { RoomDerived } from "./roomFold.ts";
import { applyRows, emptyRows, type RowsState } from "./roomRows.ts";

/// One row from `harness_actions`. Mirrors the Rust `HarnessAction`
/// (db.rs) and the live `HarnessActionEvent` (harness_action_event.rs);
/// both serialize camelCase. `payload` is an opaque JSON string whose
/// shape varies by `kind` — see docs/live-context-handover.md §6.
export interface HarnessAction {
	id: number;
	harnessId: string;
	roomId: string;
	timestampMs: number;
	kind: string;
	payload: string;
	source: string | null;
	/** #538: kind of the harness that wrote the row; null for legacy rows. */
	harnessKind?: string | null;
}

/// The Tauri event name the backend broadcasts live rows on. Global;
/// payload carries `roomId` so we filter to the active room. Exported
/// for App's cross-room api_error watcher (D2f) — errors don't flow
/// through harnessActivity, so the toast/badge surfaces tap this.
export const ACTION_EVENT = "harness-action";

/// Upper bound on the initial backfill load (newest-N by timestamp).
/// Kept modest (#165): every active room's Live Context stays mounted,
/// so loading thousands of rows per room costs memory, the feed
/// transform, and a Virtuoso re-measure when a heavy room is switched
/// to. 500 recent rows is plenty for "what's happening"; lazy-loading
/// older history on scroll-up is the proper long-term fix (follow-up).
export const BACKFILL_LIMIT = 500;

/// Subscribe a component to the live action stream for `roomId`.
/// Returns the room's rows (ascending by id; trim policy in roomRows.ts), the set
/// of ids that arrived over the live event channel (everything else came
/// from the backfill query), the incremental `derived` state (plan, totals,
/// away summary, event count, newest timestamp — covering every row seen
/// since mount, not just the capped array), the receipt time of the newest
/// live arrival, and a `loading` flag true until the backfill resolves.
///
/// While `visible` is false a live event does no derived work (#593): it
/// records its id (dedupe), liveness and receipt time, and parks the row in
/// a pending buffer. Becoming visible merges + folds the buffer and sets
/// state once. Hidden consumers therefore see stale `actions`/`derived`.
///
/// `liveIds` keeps a stable identity and is mutated in lockstep with
/// `actions` updates (ids of trimmed rows are pruned) — consumers re-read
/// it on the re-render the same event causes, so it must not be used as a
/// memo dependency on its own.
///
/// Re-runs cleanly when `roomId` changes: tears down the previous
/// listener + clears rows so a room switch never shows another room's
/// activity.
export function useRoomActions(
	roomId: string | undefined,
	visible: boolean,
	opts: {
		/** false = rows only: no fold, so `derived` stays empty. For consumers
		 *  (the review pane's hunk attribution) that read `actions` alone. */
		derive?: boolean;
	} = {},
): {
	actions: HarnessAction[];
	liveIds: ReadonlySet<number>;
	loading: boolean;
	derived: RoomDerived;
	lastLiveArrivalMs: number;
} {
	const [view, setView] = useState<RowsState>(emptyRows);
	const [loading, setLoading] = useState(true);
	const stateRef = useRef<RowsState>(view);
	const pendingRef = useRef<HarnessAction[]>([]);
	const visibleRef = useRef(visible);
	visibleRef.current = visible;
	const deriveRef = useRef(opts.derive !== false);
	deriveRef.current = opts.derive !== false;
	const flushRef = useRef<(reveal?: boolean) => void>(() => {});
	// Ids ever seen this room (superset of `actions` ids), so the listener
	// can tell a genuinely new live row from a late duplicate event for a
	// queried one. Grown incrementally — rebuilding per event is O(n).
	const idsRef = useRef<Set<number>>(new Set());
	const liveIdsRef = useRef<Set<number>>(new Set());
	const lastLiveRef = useRef(0);

	useEffect(() => {
		if (!roomId) {
			stateRef.current = emptyRows();
			pendingRef.current = [];
			setView(stateRef.current);
			setLoading(false);
			return;
		}

		let cancelled = false;
		stateRef.current = emptyRows();
		pendingRef.current = [];
		setView(stateRef.current);
		idsRef.current = new Set();
		liveIdsRef.current = new Set();
		lastLiveRef.current = 0;
		setLoading(true);

		const flush = (reveal = false) => {
			if (cancelled) return;
			if (pendingRef.current.length === 0 && !reveal) return;
			// Only flushes that do work are timed (the guards above are no-ops).
			span("liveContext.flush", () => {
				const batch = pendingRef.current;
				pendingRef.current = [];
				const res = applyRows(stateRef.current, batch, { reveal, derive: deriveRef.current });
				if (res.state === stateRef.current) return;
				for (const t of res.trimmed) liveIdsRef.current.delete(t.id);
				stateRef.current = res.state;
				setView(res.state);
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
		const unlistenPromise = listen<HarnessAction>(ACTION_EVENT, (event) =>
			span("liveContext.apply", () => {
				const a = event.payload;
				if (a.roomId !== roomId || idsRef.current.has(a.id)) return;
				idsRef.current.add(a.id);
				liveIdsRef.current.add(a.id);
				lastLiveRef.current = Date.now();
				enqueue([a]);
			}),
		);

		void invoke<HarnessAction[]>("db_recent_harness_actions_by_room", {
			roomId,
			// `-1` so timestamp 0 rows (timestamp-less Claude row types
			// stamped 0 before any timestamped row) are still included.
			sinceMs: -1,
			limit: BACKFILL_LIMIT,
		})
			.then((rows) => {
				const fresh = rows.filter((a) => !idsRef.current.has(a.id));
				for (const a of fresh) idsRef.current.add(a.id);
				enqueue(fresh);
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn(`[skein] db_recent_harness_actions_by_room failed for ${roomId}:`, msg);
			})
			.finally(() => {
				if (!cancelled) setLoading(false);
			});

		return () => {
			cancelled = true;
			void unlistenPromise.then((un) => un());
		};
	}, [roomId]);

	// Reveal: fold whatever piled up while hidden before the first painted
	// frame, so the room never flashes stale content.
	useLayoutEffect(() => {
		// The card re-pins its auto-tail on reveal, so this is the moment a
		// trim past FEED_CAP is safe (see roomRows.ts).
		if (visible) flushRef.current(true);
	}, [visible]);

	return {
		actions: view.rows,
		liveIds: liveIdsRef.current,
		loading,
		derived: view.derived,
		lastLiveArrivalMs: lastLiveRef.current,
	};
}
