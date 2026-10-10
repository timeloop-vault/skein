// Pure row bookkeeping for the room's Live Context store (#593): merge
// (de-dupe + order), incremental fold, and the feed cap. No React, so it is
// unit-testable without a DOM; `useRoomActions` is a thin shell over it.

import {
	createRoomFold,
	deriveRoom,
	foldRows,
	type RoomDerived,
	type RoomFold,
} from "./roomFold.ts";
import type { HarnessAction } from "./store.ts";

// Trim policy. Virtuoso has no firstItemIndex here, so dropping rows from the
// head while the user is scrolled up would yank the viewport. Trimming is
// therefore tied to moments the card re-pins its auto-tail:
//  - on REVEAL (the flush when a hidden card becomes visible): past
//    FEED_CAP, trim to the newest FEED_KEEP (= the backfill window);
//  - while VISIBLE: only past the higher FEED_HARD_CAP, down to
//    FEED_HARD_KEEP, as a memory backstop for a card left open for days.

/// Rows kept at reveal before the array is trimmed...
export const FEED_CAP = 1000;
/// ...down to the newest this many (= the backfill window).
export const FEED_KEEP = 500;
/// Ceiling while the card is visible...
export const FEED_HARD_CAP = 2000;
/// ...down to the newest this many.
export const FEED_HARD_KEEP = 1500;

export interface RowsState {
	/** Ascending by id; at most FEED_HARD_CAP after any apply. */
	rows: HarnessAction[];
	/** Covers every row seen since mount, including rows trimmed from `rows`. */
	fold: RoomFold;
	derived: RoomDerived;
}

export interface ApplyOptions {
	/** This apply is the flush on reveal: trim at FEED_CAP, not FEED_HARD_CAP. */
	reveal?: boolean;
	/** false = rows only; the fold and `derived` are left untouched. */
	derive?: boolean;
}

export function emptyRows(): RowsState {
	const fold = createRoomFold();
	return { rows: [], fold, derived: deriveRoom(fold) };
}

/// The rows of `incoming` not already in `sorted`, ascending by id, and
/// whether they all sort after the tail. Fast path (#165, #593): live rows
/// arrive newest-last, so when every incoming id is above the tail nothing
/// can be a duplicate and the O(window) id Set is never built.
function freshRows(
	sorted: HarnessAction[],
	incoming: HarnessAction[],
): { fresh: HarnessAction[]; append: boolean } {
	const tail = sorted[sorted.length - 1];
	let fresh = incoming;
	let append = true;
	if (tail) {
		for (const a of incoming) {
			if (a.id <= tail.id) {
				append = false;
				break;
			}
		}
	}
	if (!append) {
		const seen = new Set(sorted.map((a) => a.id));
		fresh = incoming.filter((a) => !seen.has(a.id));
		// Fresh rows can still all sort after the tail (dupes were the low ones).
		append = !tail || fresh.every((a) => a.id > tail.id);
	}
	if (fresh.length > 1) {
		fresh = fresh === incoming ? [...incoming] : fresh;
		fresh.sort((a, b) => a.id - b.id);
	}
	return { fresh, append };
}

/// Insert `incoming` into `sorted` (ascending by id), skipping ids already
/// present. Returns a new array; never mutates the input.
export function mergeById(sorted: HarnessAction[], incoming: HarnessAction[]): HarnessAction[] {
	if (incoming.length === 0) return sorted;
	const { fresh, append } = freshRows(sorted, incoming);
	if (fresh.length === 0) return sorted;
	const merged = [...sorted, ...fresh];
	if (!append) merged.sort((a, b) => a.id - b.id);
	return merged;
}

/// Merge `incoming` into `state`, update the fold, then trim.
///
/// - Every fresh row newer than the current tail: `foldRows` just those
///   (incremental, O(fresh)).
/// - Otherwise (the backfill landing after live rows at mount): the fold is
///   REBUILT from the merged array. Limit: rows already trimmed are not in
///   the array, so a rebuild after a trim would forget them. That cannot
///   happen in practice — the only out-of-order source is the single mount
///   backfill, which merges before the array can reach FEED_CAP twice over —
///   and the worst case is under-counting history older than FEED_KEEP rows.
/// - Fold BEFORE trimming, so plan, totals and event count keep covering the
///   uncapped history. `derive: false` skips the fold entirely (consumers
///   that only want rows).
/// - Trim per the policy at the top of this file.
///
/// Returns the same `state` object when nothing changed. `trimmed` lists the
/// rows dropped from the array so the caller can prune per-row bookkeeping.
export function applyRows(
	state: RowsState,
	incoming: HarnessAction[],
	opts: ApplyOptions = {},
): { state: RowsState; trimmed: HarnessAction[] } {
	const { fresh, append } = freshRows(state.rows, incoming);
	const overCap = opts.reveal && state.rows.length > FEED_CAP;
	if (fresh.length === 0 && !overCap) return { state, trimmed: [] };

	let rows = fresh.length === 0 ? state.rows : [...state.rows, ...fresh];
	if (!append) rows.sort((a, b) => a.id - b.id);

	let { fold, derived } = state;
	if (opts.derive !== false && fresh.length > 0) {
		if (append) {
			foldRows(fold, fresh);
			derived = deriveRoom(fold, derived);
		} else {
			fold = createRoomFold();
			foldRows(fold, rows);
			derived = deriveRoom(fold);
		}
	}

	let trimmed: HarnessAction[] = [];
	const [cap, keep] = opts.reveal ? [FEED_CAP, FEED_KEEP] : [FEED_HARD_CAP, FEED_HARD_KEEP];
	if (rows.length > cap) {
		trimmed = rows.slice(0, rows.length - keep);
		rows = rows.slice(rows.length - keep);
	}
	return { state: { rows, fold, derived }, trimmed };
}
