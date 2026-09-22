// Room groups (#76): a two-level tab strip. Top row is one tab per
// repository (or a plain tab for a lone room); the active group's
// second row lists its rooms, main pinned first.
//
// Pure module — no React, no App.tsx wiring (a later step does that).
// Everything here operates on plain `Room[]` snapshots and returns new
// arrays (or, where noted, the same reference) rather than mutating.

import type { Room } from "./types";

/// Normalize a filesystem path for group-key comparison: backslashes
/// become forward slashes, a trailing slash is stripped, and the
/// result is lowercased. Lowercasing unconditionally is a deliberate
/// simplification — Windows and macOS both default to case-insensitive
/// filesystems, which is what every room in practice runs on, and a
/// case-sensitive Linux repo just means two paths differing only in
/// case would incorrectly collapse into one group. That's an accepted
/// edge case, not a correctness goal here.
function normalizePath(path: string): string {
	let s = path.replace(/\\/g, "/");
	if (s.length > 1 && s.endsWith("/")) {
		s = s.slice(0, -1);
	}
	return s.toLowerCase();
}

/// A room's group key: `null` for a non-git room (no `repoRoot`), the
/// normalized `repoRoot` otherwise. Two rooms group together iff their
/// keys are equal and non-null.
export function groupKey(room: Room): string | null {
	if (!room.repoRoot) {
		return null;
	}
	return normalizePath(room.repoRoot);
}

/// A room IS its group's main worktree when its own `cwd` normalizes
/// to the group `key` (i.e. `repoRoot`). A room with no `cwd` can
/// never be main.
function isMainRoom(room: Room, key: string): boolean {
	if (!room.cwd) {
		return false;
	}
	return normalizePath(room.cwd) === key;
}

/// Last non-empty path segment of a normalized key, used as the
/// group's placeholder label when there's no main room to name it
/// after in the tab strip itself.
function labelFromKey(key: string): string {
	const parts = key.split("/").filter((p) => p.length > 0);
	const last = parts[parts.length - 1];
	return last ?? key;
}

/// Whether `room`'s own `cwd` IS the group named by `key` — the same
/// test `computeSegments` uses internally to find a bucket's main
/// room, exposed here so a caller outside this module can run it
/// against a room `buildStrip` never saw (an ARCHIVED room, checked
/// by the tab strip's placeholder-lead click handler against a
/// group's key without pulling the archived list through grouping
/// itself — the strip only ever segments *active* rooms).
export function roomIsGroupMain(room: Room, key: string): boolean {
	return isMainRoom(room, key);
}

/// One top-level tab: a lone room (`plain`), or a repository group —
/// the main room as `lead` (`null` = not open, rendered as a dimmed
/// placeholder), then worktree rooms as `members` in strip order.
export type StripSegment =
	| { kind: "plain"; room: Room }
	| {
			kind: "group";
			key: string;
			label: string;
			lead: Room | null;
			members: Room[];
	  };

/// `StripSegment` plus the sort key every consumer needs: the array
/// index of the segment's first member, per the agreed ordering rule
/// ("segments ordered by the array index of their first member").
type RawSegment =
	| { kind: "plain"; room: Room; firstIndex: number }
	| {
			kind: "group";
			key: string;
			label: string;
			lead: Room | null;
			members: Room[];
			firstIndex: number;
	  };

/// Bucket `rooms` into strip segments, in final strip order. Shared by
/// every exported function below so the grouping rule (what counts as
/// a group, who leads, what order members fall in) lives in exactly
/// one place.
function computeSegments(rooms: readonly Room[]): RawSegment[] {
	const buckets = new Map<string, { room: Room; idx: number }[]>();
	const segments: RawSegment[] = [];

	rooms.forEach((room, idx) => {
		const key = groupKey(room);
		if (key === null) {
			segments.push({ kind: "plain", room, firstIndex: idx });
			return;
		}
		const bucket = buckets.get(key);
		if (bucket) {
			bucket.push({ room, idx });
		} else {
			buckets.set(key, [{ room, idx }]);
		}
	});

	for (const [key, entries] of buckets) {
		const first = entries[0];
		if (!first) {
			continue;
		}
		const mains = entries.filter((e) => isMainRoom(e.room, key));
		const nonMains = entries.filter((e) => !isMainRoom(e.room, key));

		// Exactly one room in this bucket and it IS the main → plain
		// tab, not a group (the "lone main room" case).
		if (entries.length === 1 && mains.length === 1) {
			segments.push({ kind: "plain", room: first.room, firstIndex: first.idx });
			continue;
		}

		let lead: Room | null;
		let members: Room[];
		if (mains.length > 0) {
			const [leadEntry, ...restMains] = mains;
			// mains.length > 0 guarantees leadEntry is defined; the
			// `?? null` only satisfies noUncheckedIndexedAccess.
			lead = leadEntry?.room ?? null;
			members = [...restMains.map((e) => e.room), ...nonMains.map((e) => e.room)];
		} else {
			// No main present (worktree-only bucket) → placeholder
			// lead, every room in the bucket is a member.
			lead = null;
			members = entries.map((e) => e.room);
		}

		segments.push({
			kind: "group",
			key,
			label: labelFromKey(key),
			lead,
			members,
			firstIndex: first.idx,
		});
	}

	segments.sort((a, b) => a.firstIndex - b.firstIndex);
	return segments;
}

function dropFirstIndex(seg: RawSegment): StripSegment {
	if (seg.kind === "plain") {
		return { kind: "plain", room: seg.room };
	}
	return { kind: "group", key: seg.key, label: seg.label, lead: seg.lead, members: seg.members };
}

/// Build the tab strip's top-level segments from a flat room list.
export function buildStrip(rooms: readonly Room[]): StripSegment[] {
	return computeSegments(rooms).map(dropFirstIndex);
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

/// Identifier for a strip segment, stable across rebuilds as long as
/// the underlying group key / room id doesn't change: `"g:" + key` for
/// a group, `"r:" + room.id` for a plain tab. Used by `resolveTopDrop`
/// so callers (drag-and-drop in the top row) have one string to
/// compare against without reaching into the segment's shape.
export function segmentId(seg: StripSegment): string {
	return seg.kind === "group" ? `g:${seg.key}` : `r:${seg.room.id}`;
}

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
): Room[] {
	const segments = computeSegments(rooms);
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
	if (!seg || seg.kind !== "group") {
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
