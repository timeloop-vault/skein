// Test-only reference model of the daemon's room store (#729), written from
// the "Store semantics" section of the contract, not from the code under test.

import type { RoomChange, RoomMutation } from "./roomMutations.ts";
import type { Harness, Room } from "./types.ts";

/** Reorder `items` so ids in `ids` come first in the listed order; unknown ids ignored, rest keep order. */
function reorderBy<T extends { id: string }>(items: T[], ids: readonly string[]): T[] {
	const out: T[] = [];
	const seen = new Set<string>();
	for (const id of ids) {
		const it = items.find((x) => x.id === id);
		if (it && !seen.has(id)) {
			seen.add(id);
			out.push(it);
		}
	}
	for (const it of items) if (!seen.has(it.id)) out.push(it);
	return out;
}

export function applyMutations(
	rooms: Room[],
	mutations: RoomMutation[],
): { rooms: Room[]; changes: RoomChange[] } {
	let store: Room[] = rooms.map((r) => structuredClone(r));
	const touched: string[] = [];
	const removed: string[] = [];
	let orderDirty = false;

	const touch = (id: string) => {
		if (!touched.includes(id)) touched.push(id);
	};
	const find = (id: string) => store.find((r) => r.id === id);

	for (const m of mutations) {
		switch (m.type) {
			case "createRoom": {
				if (find(m.room.id)) break;
				store.push(structuredClone(m.room));
				touch(m.room.id);
				const ri = removed.indexOf(m.room.id);
				if (ri >= 0) removed.splice(ri, 1);
				orderDirty = true;
				break;
			}
			case "updateRoom": {
				const cur = find(m.room.id);
				if (!cur) break;
				const next: Room = { ...structuredClone(m.room), harnesses: cur.harnesses };
				store = store.map((r) => (r.id === cur.id ? next : r));
				touch(cur.id);
				break;
			}
			case "removeRoom": {
				if (!find(m.roomId)) break;
				store = store.filter((r) => r.id !== m.roomId);
				const ti = touched.indexOf(m.roomId);
				if (ti >= 0) touched.splice(ti, 1);
				if (!removed.includes(m.roomId)) removed.push(m.roomId);
				orderDirty = true;
				break;
			}
			case "reorderRooms":
				store = reorderBy(store, m.roomIds);
				orderDirty = true;
				break;
			case "putHarness": {
				const room = find(m.roomId);
				if (!room) break;
				const h: Harness = structuredClone(m.harness);
				const i = room.harnesses.findIndex((x) => x.id === h.id);
				if (i >= 0) room.harnesses[i] = h;
				else room.harnesses.push(h);
				touch(room.id);
				break;
			}
			case "removeHarness": {
				const room = find(m.roomId);
				if (!room) break;
				if (!room.harnesses.some((x) => x.id === m.harnessId)) break;
				room.harnesses = room.harnesses.filter((x) => x.id !== m.harnessId);
				touch(room.id);
				break;
			}
			case "reorderHarnesses": {
				const room = find(m.roomId);
				if (!room) break;
				room.harnesses = reorderBy(room.harnesses, m.harnessIds);
				touch(room.id);
				break;
			}
		}
	}

	const changes: RoomChange[] = [];
	for (const id of touched) {
		const r = find(id);
		if (r) changes.push({ type: "roomPut", room: structuredClone(r) });
	}
	for (const id of removed) changes.push({ type: "roomRemoved", roomId: id });
	if (orderDirty) changes.push({ type: "roomsOrder", roomIds: store.map((r) => r.id) });
	return { rooms: store, changes };
}
