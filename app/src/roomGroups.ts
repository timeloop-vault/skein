// Room groups (#76): a two-level tab strip. Top row is one tab per
// repository (or a plain tab for a lone room); the active group's
// second row lists its rooms, main pinned first.
//
// Pure module — no React, no App.tsx wiring (a later step does that).
// Everything here operates on plain `Room[]` snapshots and returns new
// arrays (or, where noted, the same reference) rather than mutating.

import { type RawSegment, type StripSegment, computeSegments } from "./roomSegments";
import type { Room } from "./types";

export { resolveRowDrop, resolveTopDrop } from "./roomDrop";
export { groupKey, roomIsGroupMain, type StripSegment } from "./roomSegments";

function dropFirstIndex(seg: RawSegment): StripSegment {
	if (seg.kind === "plain") {
		return { kind: "plain", room: seg.room };
	}
	return { kind: "group", key: seg.key, label: seg.label, lead: seg.lead, members: seg.members };
}

/// Build the tab strip's top-level segments from a flat room list.
/// `ungrouped` (#418) names rooms that always render as plain tabs.
export function buildStrip(
	rooms: readonly Room[],
	ungrouped?: ReadonlySet<string>,
): StripSegment[] {
	return computeSegments(rooms, ungrouped).map(dropFirstIndex);
}

/// Every room a group segment holds, lead first (if present) then
/// members in strip order. Callers fold this list through
/// `harnessActivity.ts`'s `higherPriorityStatus` to get a group-level
/// status without this module reimplementing status priority.
export function groupRooms(seg: StripSegment): Room[] {
	if (seg.kind === "plain") {
		return [seg.room];
	}
	return seg.lead ? [seg.lead, ...seg.members] : [...seg.members];
}

/// The top-row `GroupTab`'s display name (#241): the main room's own
/// `Room.name` when it's open (`seg.lead`), the repo-derived `label`
/// otherwise. Renaming either the GroupTab or the lead's own second-row
/// tab writes the SAME `Room.name` field — this only decides which
/// value currently stands in for the group as a whole, not a separate
/// piece of state.
export function groupDisplayName(seg: Extract<StripSegment, { kind: "group" }>): string {
	return seg.lead ? seg.lead.name : seg.label;
}

/// Every room in strip order — a group contributes its lead (if open)
/// then its members. Used by next/prev room and Alt+1-9: there is no
/// collapse any more, so unlike the old `visibleRoomOrder` this never
/// skips a room.
export function allRoomOrder(segments: readonly StripSegment[]): Room[] {
	const out: Room[] = [];
	for (const seg of segments) {
		out.push(...groupRooms(seg));
	}
	return out;
}

/// Which segment `roomId` is currently in — as a plain tab, a group
/// lead, or a group member — used to derive the active group from the
/// active room. `undefined` if no segment holds it (an archived or
/// unknown room).
export function segmentOfRoom(
	segments: readonly StripSegment[],
	roomId: string,
): StripSegment | undefined {
	return segments.find((seg) => {
		if (seg.kind === "plain") {
			return seg.room.id === roomId;
		}
		return seg.lead?.id === roomId || seg.members.some((m) => m.id === roomId);
	});
}

/// The room a click on `seg`'s top-level tab (or Alt+N) should land
/// on. A plain tab always goes to its own room. A group goes to the
/// last room used in that group (`lastUsedByGroup`, keyed on group
/// `key`) as long as that id is STILL a member or the lead of `seg` —
/// a stale id (its room left the group, or was closed) falls back to
/// the lead, and a group with no lead open falls back to its first
/// member. `rooms` supplies the up-to-date Room object for the
/// last-used id; `seg`'s own lead/members decide whether that id is
/// still valid. `null` only for an empty placeholder group, which
/// `computeSegments` never actually produces (a group always has at
/// least one member), but the return type stays honest about it.
export function topLevelTarget(
	seg: StripSegment,
	lastUsedByGroup: ReadonlyMap<string, string>,
	rooms: readonly Room[],
): Room | null {
	if (seg.kind === "plain") {
		return seg.room;
	}
	const lastUsedId = lastUsedByGroup.get(seg.key);
	if (lastUsedId) {
		const stillValid = seg.lead?.id === lastUsedId || seg.members.some((m) => m.id === lastUsedId);
		if (stillValid) {
			const room = rooms.find((r) => r.id === lastUsedId);
			if (room) {
				return room;
			}
		}
	}
	if (seg.lead) {
		return seg.lead;
	}
	return seg.members[0] ?? null;
}

/// Where to land after closing `closedId`, computed against `rooms`
/// (the active list from BEFORE the close — it still contains the
/// closed room, so its segment can be found). Closing a room inside a
/// group that still has another open room stays in that group: the
/// lead if it's open and isn't the one closing, else the nearest
/// neighbour in the second row (`groupRooms(seg)` order — left, then
/// right). Closing a plain room, or the last open room of a group,
/// hops to the adjacent top-level segment instead (pre-close order —
/// left, then right), resolved through `topLevelTarget` so a group
/// neighbour honours `lastUsedByGroup`. `null` if there's no other
/// room to land on. A `closedId` not found in any segment (shouldn't
/// happen — the caller only calls this for a room it just saw close)
/// falls back to the first other room in `allRoomOrder`.
export function nextActiveAfterClose(
	rooms: readonly Room[],
	closedId: string,
	lastUsedByGroup: ReadonlyMap<string, string>,
): Room | null {
	const segments = buildStrip(rooms);
	const segIndex = segments.findIndex((seg) => {
		if (seg.kind === "plain") {
			return seg.room.id === closedId;
		}
		return seg.lead?.id === closedId || seg.members.some((m) => m.id === closedId);
	});

	if (segIndex === -1) {
		const fallback = allRoomOrder(segments).find((r) => r.id !== closedId);
		return fallback ?? null;
	}

	const seg = segments[segIndex];
	if (seg?.kind === "group") {
		const row = groupRooms(seg);
		const stillOpen = row.filter((r) => r.id !== closedId);
		if (stillOpen.length > 0) {
			if (seg.lead && seg.lead.id !== closedId) {
				return seg.lead;
			}
			const closedIdx = row.findIndex((r) => r.id === closedId);
			return row[closedIdx - 1] ?? row[closedIdx + 1] ?? null;
		}
	}

	// Plain segment, or the last open room of its group: hop to the
	// nearest adjacent top-level segment instead.
	for (const adjacent of [segments[segIndex - 1], segments[segIndex + 1]]) {
		if (!adjacent) {
			continue;
		}
		const target = topLevelTarget(adjacent, lastUsedByGroup, rooms);
		if (target && target.id !== closedId) {
			return target;
		}
	}
	return null;
}

/// Identifier for a strip segment, stable across rebuilds as long as
/// the underlying group key / room id doesn't change: `"g:" + key` for
/// a group, `"r:" + room.id` for a plain tab. Used by `resolveTopDrop`
/// so callers (drag-and-drop in the top row) have one string to
/// compare against without reaching into the segment's shape.
export function segmentId(seg: StripSegment): string {
	return seg.kind === "group" ? `g:${seg.key}` : `r:${seg.room.id}`;
}
