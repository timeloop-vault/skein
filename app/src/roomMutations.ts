// Wire types for per-room mutations (#729) and the diff that produces them.
// Mirrors skein-proto; camelCase, discriminated on `type`.

import type { Harness, Room } from "./types";

export type RoomMutation =
	| { type: "createRoom"; room: Room }
	| { type: "updateRoom"; room: Room }
	| { type: "removeRoom"; roomId: string }
	| { type: "reorderRooms"; roomIds: string[] }
	| { type: "putHarness"; roomId: string; harness: Harness }
	| { type: "removeHarness"; roomId: string; harnessId: string }
	| { type: "reorderHarnesses"; roomId: string; harnessIds: string[] };

export type RoomChange =
	| { type: "roomPut"; room: Room }
	| { type: "roomRemoved"; roomId: string }
	| { type: "roomsOrder"; roomIds: string[] };

export interface RoomsChanged {
	origin: string;
	changes: RoomChange[];
}

export interface RoomsApplyOutcome {
	skipped: { index: number; reason: string }[];
}

export const ROOMS_CHANGED_EVENT = "skein://rooms-changed";

/** Key-order-insensitive deep equality; `undefined` and a missing key are equal. */
export function deepEqual(a: unknown, b: unknown): boolean {
	if (a === b) return true;
	if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) {
		return false;
	}
	if (Array.isArray(a) || Array.isArray(b)) {
		if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
		return a.every((v, i) => deepEqual(v, b[i]));
	}
	const ra = a as Record<string, unknown>;
	const rb = b as Record<string, unknown>;
	const keys = new Set([...Object.keys(ra), ...Object.keys(rb)]);
	for (const k of keys) {
		if (!deepEqual(ra[k], rb[k])) return false;
	}
	return true;
}

/**
 * Harness fields the daemon does not store (crates/skein-proto/src/rooms.rs has
 * no counterpart), so they are never synced and a remote room never carries
 * them. The list must grow if a client-only field is ever added. There is no
 * local-only key at room level.
 */
export const LOCAL_ONLY_HARNESS_KEYS = ["spawnGen"] as const;

/** Harness equality that ignores the client-only keys. */
export function harnessEqual(a: Harness, b: Harness): boolean {
	const ra = { ...a } as Record<string, unknown>;
	const rb = { ...b } as Record<string, unknown>;
	for (const k of LOCAL_ONLY_HARNESS_KEYS) {
		delete ra[k];
		delete rb[k];
	}
	return deepEqual(ra, rb);
}

function sameIds(a: readonly string[], b: readonly string[]): boolean {
	return a.length === b.length && a.every((id, i) => id === b[i]);
}

function roomLevelEqual(a: Room, b: Room): boolean {
	const { harnesses: _a, ...ra } = a;
	const { harnesses: _b, ...rb } = b;
	return deepEqual(ra, rb);
}

function diffHarnesses(prev: Room, next: Room, out: RoomMutation[]): void {
	const prevById = new Map(prev.harnesses.map((h) => [h.id, h]));
	const nextIds = new Set(next.harnesses.map((h) => h.id));
	for (const h of prev.harnesses) {
		if (!nextIds.has(h.id)) {
			out.push({ type: "removeHarness", roomId: next.id, harnessId: h.id });
		}
	}
	for (const h of next.harnesses) {
		const old = prevById.get(h.id);
		if (old === h) continue;
		if (old === undefined || !harnessEqual(old, h)) {
			out.push({ type: "putHarness", roomId: next.id, harness: h });
		}
	}
	const resulting = [
		...prev.harnesses.map((h) => h.id).filter((id) => nextIds.has(id)),
		...next.harnesses.map((h) => h.id).filter((id) => !prevById.has(id)),
	];
	const wanted = next.harnesses.map((h) => h.id);
	if (!sameIds(resulting, wanted)) {
		out.push({ type: "reorderHarnesses", roomId: next.id, harnessIds: wanted });
	}
}

export function diffRooms(prev: readonly Room[], next: readonly Room[]): RoomMutation[] {
	const prevById = new Map(prev.map((r) => [r.id, r]));
	const nextById = new Map(next.map((r) => [r.id, r]));
	const out: RoomMutation[] = [];

	for (const r of prev) {
		if (!nextById.has(r.id)) out.push({ type: "removeRoom", roomId: r.id });
	}
	for (const r of next) {
		if (!prevById.has(r.id)) out.push({ type: "createRoom", room: r });
	}
	for (const r of next) {
		const old = prevById.get(r.id);
		if (old === undefined || old === r) continue;
		if (!roomLevelEqual(old, r)) out.push({ type: "updateRoom", room: r });
		diffHarnesses(old, r, out);
	}

	const resulting = [
		...prev.map((r) => r.id).filter((id) => nextById.has(id)),
		...next.map((r) => r.id).filter((id) => !prevById.has(id)),
	];
	const wanted = next.map((r) => r.id);
	if (!sameIds(resulting, wanted)) out.push({ type: "reorderRooms", roomIds: wanted });
	return out;
}
