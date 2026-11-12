// Applies daemon room changes (#729) to the local rooms state. Idempotent, and
// keeps object identity for anything that did not change.

import { deepEqual, harnessEqual, LOCAL_ONLY_HARNESS_KEYS, type RoomChange } from "./roomMutations";
import type { Harness, Room } from "./types";

function mergeHarnesses(local: readonly Harness[], remote: readonly Harness[]): Harness[] {
	const localById = new Map(local.map((h) => [h.id, h]));
	const merged = remote.map((h) => {
		const old = localById.get(h.id);
		if (old === undefined) return h;
		if (harnessEqual(old, h)) return old;
		// Replace: the remote harness is the truth, except the client-only keys.
		const m: Record<string, unknown> = { ...h };
		const o = old as unknown as Record<string, unknown>;
		for (const k of LOCAL_ONLY_HARNESS_KEYS) {
			if (o[k] !== undefined) m[k] = o[k];
		}
		return m as unknown as Harness;
	});
	const same = merged.length === local.length && merged.every((h, i) => h === local[i]);
	return same ? (local as Harness[]) : merged;
}

function mergeRoom(local: Room, remote: Room): Room {
	// Replace: absent keys in the remote mean absent.
	const merged: Room = {
		...remote,
		harnesses: mergeHarnesses(local.harnesses, remote.harnesses),
	};
	return deepEqual(merged, local) ? local : merged;
}

function reorder(rooms: readonly Room[], ids: readonly string[]): readonly Room[] {
	const byId = new Map(rooms.map((r) => [r.id, r]));
	const listed = new Set<string>();
	const out: Room[] = [];
	for (const id of ids) {
		const r = byId.get(id);
		if (r !== undefined && !listed.has(id)) {
			listed.add(id);
			out.push(r);
		}
	}
	for (const r of rooms) {
		if (!listed.has(r.id)) out.push(r);
	}
	return out.every((r, i) => r === rooms[i]) ? rooms : out;
}

export function applyRoomChanges(rooms: readonly Room[], changes: readonly RoomChange[]): Room[] {
	let cur: readonly Room[] = rooms;
	for (const c of changes) {
		switch (c.type) {
			case "roomPut": {
				const i = cur.findIndex((r) => r.id === c.room.id);
				if (i < 0) {
					cur = [...cur, c.room];
					break;
				}
				const local = cur[i];
				if (local === undefined) break;
				const merged = mergeRoom(local, c.room);
				if (merged !== local) cur = cur.map((r, j) => (j === i ? merged : r));
				break;
			}
			case "roomRemoved":
				if (cur.some((r) => r.id === c.roomId)) cur = cur.filter((r) => r.id !== c.roomId);
				break;
			case "roomsOrder":
				cur = reorder(cur, c.roomIds);
				break;
		}
	}
	return cur === rooms ? (rooms as Room[]) : [...cur];
}
