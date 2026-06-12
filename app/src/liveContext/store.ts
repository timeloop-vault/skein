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
import { useEffect, useRef, useState } from "react";

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
}

/// The Tauri event name the backend broadcasts live rows on. Global;
/// payload carries `roomId` so we filter to the active room.
const ACTION_EVENT = "harness-action";

/// Upper bound on the initial backfill load (newest-N by timestamp).
/// Recon-scale rooms can have ≥6k rows; virtualization + a larger or
/// paged load is deferred to D2g, so until then the backfill banner's
/// window describes the newest 5000, not necessarily the room's start.
const BACKFILL_LIMIT = 5000;

/// Insert `incoming` into `sorted` (ascending by id), skipping ids
/// already present. Returns a new array; never mutates the input.
function mergeById(sorted: HarnessAction[], incoming: HarnessAction[]): HarnessAction[] {
	if (incoming.length === 0) return sorted;
	const seen = new Set(sorted.map((a) => a.id));
	const fresh = incoming.filter((a) => !seen.has(a.id));
	if (fresh.length === 0) return sorted;
	const merged = [...sorted, ...fresh];
	merged.sort((a, b) => a.id - b.id);
	return merged;
}

/// Subscribe a component to the live action stream for `roomId`.
/// Returns the room's rows (ascending by id), the set of ids that
/// arrived over the live event channel (everything else came from the
/// backfill query), and a `loading` flag that is true until the initial
/// backfill query resolves.
///
/// `liveIds` keeps a stable identity and is mutated in lockstep with
/// `actions` updates — consumers re-read it on the re-render the same
/// event causes, so it must not be used as a memo dependency on its own.
///
/// Re-runs cleanly when `roomId` changes: tears down the previous
/// listener + clears rows so a room switch never shows another room's
/// activity.
export function useRoomActions(roomId: string | undefined): {
	actions: HarnessAction[];
	liveIds: ReadonlySet<number>;
	loading: boolean;
} {
	const [actions, setActions] = useState<HarnessAction[]>([]);
	const [loading, setLoading] = useState(true);
	// Hold the latest rows in a ref so the live-event closure can merge
	// against current state without being re-created on every append.
	const actionsRef = useRef<HarnessAction[]>([]);
	actionsRef.current = actions;
	// Ids ever seen this room (superset of `actions` ids), so the listener
	// can tell a genuinely new live row from a late duplicate event for a
	// queried one. Grown incrementally — rebuilding per event is O(n).
	const idsRef = useRef<Set<number>>(new Set());
	const liveIdsRef = useRef<Set<number>>(new Set());

	useEffect(() => {
		if (!roomId) {
			setActions([]);
			setLoading(false);
			return;
		}

		let cancelled = false;
		setActions([]);
		idsRef.current = new Set();
		liveIdsRef.current = new Set();
		setLoading(true);

		const apply = (incoming: HarnessAction[]) => {
			if (cancelled) return;
			for (const a of incoming) idsRef.current.add(a.id);
			const next = mergeById(actionsRef.current, incoming);
			if (next !== actionsRef.current) {
				actionsRef.current = next;
				setActions(next);
			}
		};

		// Attach the listener BEFORE the backfill query so a row
		// inserted in the gap still lands (merge de-dupes if the query
		// also returned it).
		const unlistenPromise = listen<HarnessAction>(ACTION_EVENT, (event) => {
			if (event.payload.roomId === roomId) {
				if (!idsRef.current.has(event.payload.id)) {
					liveIdsRef.current.add(event.payload.id);
				}
				apply([event.payload]);
			}
		});

		void invoke<HarnessAction[]>("db_recent_harness_actions_by_room", {
			roomId,
			// `-1` so timestamp 0 rows (timestamp-less Claude row types
			// stamped 0 before any timestamped row) are still included.
			sinceMs: -1,
			limit: BACKFILL_LIMIT,
		})
			.then((rows) => {
				apply(rows);
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

	return { actions, liveIds: liveIdsRef.current, loading };
}
