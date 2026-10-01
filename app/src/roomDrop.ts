// Drag-and-drop reordering for the room strip (#76, split from
// roomGroups.ts in #461). Pure: both resolvers return a new room array,
// or the same reference when the drop is a no-op.

import { computeSegments, type RawSegment } from "./roomSegments";
import type { Room } from "./types";

function rawSegmentId(seg: RawSegment): string {
	return seg.kind === "group" ? `g:${seg.key}` : `r:${seg.room.id}`;
}

function flattenSegments(segments: readonly RawSegment[]): Room[] {
	const out: Room[] = [];
	for (const seg of segments) {
		if (seg.kind === "plain") {
			out.push(seg.room);
			continue;
		}
		if (seg.lead) {
			out.push(seg.lead);
		}
		out.push(...seg.members);
	}
	return out;
}

/// Top-row drag: reorder whole top-level tabs — a plain room, or every
/// room in a group, staying contiguous and keeping their relative
/// order — before or after another segment. Both ids are
/// `segmentId`-shaped strings ("g:<key>" / "r:<roomId>"), resolved
/// fresh against `rooms` rather than trusted from drag start. No-op
/// (same array reference back): either id doesn't resolve to a
/// segment (this is also what refuses a drop onto a bare second-row
/// room id, since that string is never segment-shaped), or they
/// resolve to the same one.
export function resolveTopDrop(
	rooms: readonly Room[],
	dragSegId: string,
	targetSegId: string,
	place: "before" | "after",
	ungrouped?: ReadonlySet<string>,
): Room[] {
	const segments = computeSegments(rooms, ungrouped);
	const fromIndex = segments.findIndex((s) => rawSegmentId(s) === dragSegId);
	const toIndex = segments.findIndex((s) => rawSegmentId(s) === targetSegId);
	if (fromIndex === -1 || toIndex === -1 || fromIndex === toIndex) {
		return rooms as Room[];
	}

	const moving = segments[fromIndex];
	if (!moving) {
		return rooms as Room[];
	}
	const withoutMoving = segments.filter((_, i) => i !== fromIndex);
	const targetNewIndex = withoutMoving.findIndex((s) => rawSegmentId(s) === targetSegId);
	if (targetNewIndex === -1) {
		return rooms as Room[];
	}
	const insertAt = place === "before" ? targetNewIndex : targetNewIndex + 1;
	const newSegments = [
		...withoutMoving.slice(0, insertAt),
		moving,
		...withoutMoving.slice(insertAt),
	];
	return flattenSegments(newSegments);
}

/// Second-row drag: reorder a non-lead member within its OWN group,
/// inserting it immediately before (or after) `targetRoomId` among
/// that group's members — `"after"` is the only way to drag a member
/// into the group's last slot, since there's no next member to name as
/// a `"before"` target. No-op (returns the same array reference) if
/// `roomId` isn't a non-lead member of some group, if `targetRoomId`
/// isn't a member of THAT SAME group, or if either id names a lead —
/// the lead (real or placeholder) is pinned first by which room IS the
/// main worktree, not by drag order, and is never a drop target to go
/// before.
export function resolveRowDrop(
	rooms: readonly Room[],
	dragRoomId: string,
	targetRoomId: string,
	place: "before" | "after",
): Room[] {
	if (dragRoomId === targetRoomId) {
		return rooms as Room[];
	}
	const segments = computeSegments(rooms);
	const segIndex = segments.findIndex(
		(s) => s.kind === "group" && s.members.some((m) => m.id === dragRoomId),
	);
	if (segIndex === -1) {
		return rooms as Room[];
	}
	const seg = segments[segIndex];
	if (seg?.kind !== "group") {
		return rooms as Room[];
	}
	if (seg.lead && (seg.lead.id === dragRoomId || seg.lead.id === targetRoomId)) {
		return rooms as Room[];
	}
	if (!seg.members.some((m) => m.id === targetRoomId)) {
		return rooms as Room[];
	}

	const moved = seg.members.find((m) => m.id === dragRoomId);
	if (!moved) {
		return rooms as Room[];
	}
	const remaining = seg.members.filter((m) => m.id !== dragRoomId);
	const targetIdx = remaining.findIndex((m) => m.id === targetRoomId);
	const insertAt = place === "before" ? targetIdx : targetIdx + 1;
	const newMembers = [...remaining.slice(0, insertAt), moved, ...remaining.slice(insertAt)];

	const newSegments = segments.slice();
	newSegments[segIndex] = { ...seg, members: newMembers };
	return flattenSegments(newSegments);
}
