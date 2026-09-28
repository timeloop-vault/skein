// Pure logic behind Reopen's list (#417): filtering, multi-select and
// retire/unretire. No React, so it is table-testable.

import type { Room } from "./types.ts";

export const matchesQuery = (room: Room, query: string): boolean => {
	const q = query.trim().toLowerCase();
	if (!q) return true;
	return [room.name, room.repo, room.branch, room.cwd].some((f) => f?.toLowerCase().includes(q));
};

/** Archived rooms matching the query, in input order. Retired ones are
 *  left out unless `showRetired` (then both kinds show; retired ones are
 *  told apart by their `retired` field). */
export const visibleArchived = (
	rooms: readonly Room[],
	query: string,
	showRetired: boolean,
): Room[] =>
	rooms.filter(
		(r) =>
			r.archived !== undefined &&
			(showRetired || r.retired === undefined) &&
			matchesQuery(r, query),
	);

export interface Selection {
	readonly ids: ReadonlySet<string>;
	/** Where a Shift-extend starts from; null = nothing clicked yet. */
	readonly anchor: string | null;
}

export const emptySelection: Selection = { ids: new Set(), anchor: null };

/** Plain click: select only this row and make it the anchor. */
export const click = (_sel: Selection, id: string): Selection => ({
	ids: new Set([id]),
	anchor: id,
});

/** Ctrl/Cmd-click: flip one row, keeping the rest; it becomes the anchor. */
export const toggle = (sel: Selection, id: string): Selection => {
	const ids = new Set(sel.ids);
	if (ids.has(id)) ids.delete(id);
	else ids.add(id);
	return { ids, anchor: id };
};

/** Shift-click / Shift+arrow: the range anchor..id replaces the previous
 *  range (the anchor stays). With no usable anchor it acts like a click. */
export const extendTo = (sel: Selection, orderedIds: readonly string[], id: string): Selection => {
	const to = orderedIds.indexOf(id);
	const from = sel.anchor === null ? -1 : orderedIds.indexOf(sel.anchor);
	if (to < 0 || from < 0) return click(sel, id);
	const [lo, hi] = from <= to ? [from, to] : [to, from];
	return { ids: new Set(orderedIds.slice(lo, hi + 1)), anchor: sel.anchor };
};

export const selectAll = (orderedIds: readonly string[]): Selection => ({
	ids: new Set(orderedIds),
	anchor: orderedIds[0] ?? null,
});

/** Drop ids that are no longer visible (the filter narrowed). */
export const pruneTo = (sel: Selection, orderedIds: readonly string[]): Selection => {
	const visible = new Set(orderedIds);
	const ids = new Set([...sel.ids].filter((i) => visible.has(i)));
	const anchor = sel.anchor !== null && visible.has(sel.anchor) ? sel.anchor : null;
	if (ids.size === sel.ids.size && anchor === sel.anchor) return sel;
	return { ids, anchor };
};

/** Stamp `retired: now` on archived rooms in `ids`; never on an open room. */
export const retireRooms = (rooms: Room[], ids: readonly string[], now: number): Room[] => {
	const set = new Set(ids);
	return rooms.map((r) => (set.has(r.id) && r.archived !== undefined ? { ...r, retired: now } : r));
};

/** Remove the `retired` key (absent, not undefined) from rooms in `ids`. */
export const unretireRooms = (rooms: Room[], ids: readonly string[]): Room[] => {
	const set = new Set(ids);
	return rooms.map((r) => {
		if (!set.has(r.id) || r.retired === undefined) return r;
		const { retired: _retired, ...rest } = r;
		return rest;
	});
};
