// Segment bucketing for the room strip (#76, split from roomGroups.ts
// in #461): the grouping rule — what counts as a group, who leads, what
// order members fall in — lives here once, shared by the strip builder
// (roomGroups.ts) and the drag-drop resolvers (roomDrop.ts).

import type { Room } from "./types";

/// Normalize a filesystem path for group-key comparison: backslashes
/// become forward slashes, a trailing slash is stripped, and the
/// result is lowercased. Lowercasing unconditionally is a deliberate
/// simplification — Windows and macOS both default to case-insensitive
/// filesystems, which is what every room in practice runs on, and a
/// case-sensitive Linux repo just means two paths differing only in
/// case would incorrectly collapse into one group. That's an accepted
/// edge case, not a correctness goal here.
export function normalizePath(path: string): string {
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
export type RawSegment =
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
export function computeSegments(
	rooms: readonly Room[],
	ungrouped?: ReadonlySet<string>,
): RawSegment[] {
	const buckets = new Map<string, { room: Room; idx: number }[]>();
	const segments: RawSegment[] = [];

	rooms.forEach((room, idx) => {
		// #418: an `ungrouped` room (its folder now holds a different
		// repository) must not join the group of whatever repo is there.
		const key = ungrouped?.has(room.id) ? null : groupKey(room);
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
