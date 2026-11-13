// Three-way merge of a remote room change into local state (#729). The
// baseline is what the daemon is known to hold; a local value that differs
// from it is an edit not yet synced, and a remote change never overwrites it.
// Keeps object identity for anything unchanged.

import { applyRoomChanges } from "./roomChanges";
import {
	DAEMON_OWNED_HARNESS_KEYS,
	deepEqual,
	LOCAL_ONLY_HARNESS_KEYS,
	type RoomChange,
} from "./roomMutations";
import type { Harness, Room } from "./types";

function orderOf(rooms: readonly Room[], other: readonly Room[]): string[] {
	const ids = new Set(other.map((r) => r.id));
	return rooms.filter((r) => ids.has(r.id)).map((r) => r.id);
}

function sameIds(a: readonly string[], b: readonly string[]): boolean {
	return a.length === b.length && a.every((id, i) => id === b[i]);
}

/**
 * Per-key three-way merge over the union of keys. A local value that differs
 * from the baseline is a pending edit and wins; otherwise the remote value is
 * taken, and the key is dropped when the remote lacks it. `localOnly` keys
 * always keep the local value; `remoteOwned` keys (daemon-owned, #755) always
 * take the remote value, even when local differs, and are dropped when the
 * remote lacks them. Never assigns `undefined`.
 */
function mergeKeys(
	base: object,
	local: object,
	remote: object,
	localOnly: readonly string[],
	remoteOwned: readonly string[] = [],
): Record<string, unknown> {
	const b = base as Record<string, unknown>;
	const l = local as Record<string, unknown>;
	const r = remote as Record<string, unknown>;
	const out: Record<string, unknown> = {};
	for (const k of new Set([...Object.keys(b), ...Object.keys(l), ...Object.keys(r)])) {
		const v = remoteOwned.includes(k)
			? r[k]
			: localOnly.includes(k) || !deepEqual(l[k], b[k])
				? l[k]
				: r[k];
		if (v !== undefined) out[k] = v;
	}
	return out;
}

function rebaseHarnesses(
	base: readonly Harness[],
	local: readonly Harness[],
	remote: readonly Harness[],
): Harness[] {
	const baseById = new Map(base.map((h) => [h.id, h]));
	const localById = new Map(local.map((h) => [h.id, h]));
	const remoteIds = new Set(remote.map((h) => h.id));
	const out: Harness[] = [];
	for (const rh of remote) {
		const lh = localById.get(rh.id);
		const bh = baseById.get(rh.id);
		if (lh === undefined) {
			// Absent locally but in the baseline: a pending local removal wins.
			if (bh === undefined) out.push(rh);
			continue;
		}
		if (bh === undefined) {
			out.push(lh);
			continue;
		}
		const m = mergeKeys(
			bh,
			lh,
			rh,
			LOCAL_ONLY_HARNESS_KEYS,
			DAEMON_OWNED_HARNESS_KEYS,
		) as unknown as Harness;
		out.push(deepEqual(m, lh) ? lh : m);
	}
	// Pending local add: not in the baseline, so the remote cannot have dropped
	// it. Each sits after the nearest preceding local harness that survived the
	// merge, so the user's order is kept.
	local.forEach((lh, i) => {
		if (remoteIds.has(lh.id) || baseById.has(lh.id)) return;
		let at = 0;
		for (let j = i - 1; j >= 0; j--) {
			const prev = local[j];
			const k = prev === undefined ? -1 : out.findIndex((h) => h.id === prev.id);
			if (k >= 0) {
				at = k + 1;
				break;
			}
		}
		out.splice(at, 0, lh);
	});
	const baseOrder = base.filter((h) => localById.has(h.id)).map((h) => h.id);
	const localOrder = local.filter((h) => baseById.has(h.id)).map((h) => h.id);
	let result = out;
	if (!sameIds(baseOrder, localOrder)) {
		// Pending local reorder: keep the local order.
		const pos = new Map(local.map((h, i) => [h.id, i]));
		result = [...out].sort(
			(a, b) =>
				(pos.get(a.id) ?? Number.MAX_SAFE_INTEGER) - (pos.get(b.id) ?? Number.MAX_SAFE_INTEGER),
		);
	}
	const same = result.length === local.length && result.every((h, i) => h === local[i]);
	return same ? (local as Harness[]) : result;
}

function rebaseRoom(base: Room | undefined, local: Room, remote: Room): Room {
	// Created locally and not yet synced: everything about it is pending.
	if (base === undefined) return local;
	const { harnesses: _h, ...rest } = local;
	const merged = mergeKeys(
		{ ...base, harnesses: undefined },
		{ ...rest },
		{ ...remote, harnesses: undefined },
		[],
	);
	merged.harnesses = rebaseHarnesses(base.harnesses, local.harnesses, remote.harnesses);
	const room = merged as unknown as Room;
	return deepEqual(room, local) ? local : room;
}

function rebaseOne(base: readonly Room[], local: readonly Room[], c: RoomChange): readonly Room[] {
	switch (c.type) {
		case "roomPut": {
			const i = local.findIndex((r) => r.id === c.room.id);
			const b = base.find((r) => r.id === c.room.id);
			if (i < 0) {
				// In the baseline but gone locally: the pending removal wins.
				return b === undefined ? [...local, c.room] : local;
			}
			const cur = local[i];
			if (cur === undefined) return local;
			const merged = rebaseRoom(b, cur, c.room);
			return merged === cur ? local : local.map((r, j) => (j === i ? merged : r));
		}
		case "roomRemoved":
			return local.some((r) => r.id === c.roomId) ? local.filter((r) => r.id !== c.roomId) : local;
		case "roomsOrder":
			// A local order that already differs from the baseline is a pending reorder.
			return sameIds(orderOf(local, base), orderOf(base, local))
				? applyRoomChanges(local, [c])
				: local;
	}
}

export function rebaseRemote(
	baseline: readonly Room[],
	local: readonly Room[],
	changes: readonly RoomChange[],
): { baseline: Room[]; local: Room[] } {
	let base: readonly Room[] = baseline;
	let cur: readonly Room[] = local;
	for (const c of changes) {
		cur = rebaseOne(base, cur, c);
		base = applyRoomChanges(base, [c]);
	}
	return {
		baseline: base === baseline ? (baseline as Room[]) : [...base],
		local: cur === local ? (local as Room[]) : [...cur],
	};
}
