// #294: pure helpers behind an OS-notification click jump. Split out
// of App.tsx's click-listener glue (window focus, unarchiving,
// setRooms) so the actual decisions — "does this room/harness still
// exist", "should this event trigger a fresh drain" — are testable
// without a DOM or a Tauri window.

import type { Room } from "./types.ts";

export type ClickTarget = { roomId: string; harnessId: string };

export type JumpDecision = { found: false } | { found: true; hasHarness: boolean };

/** Where an OS-notification click should land, given the rooms known
 * right now. `found: false` covers the room having been closed and
 * deleted since the banner fired (#118) — the caller does nothing.
 * `hasHarness: false` means the harness itself is gone but the room
 * survives, so the caller still switches to the room without touching
 * `activeHarnessId`. */
export function resolveClickTarget(rooms: readonly Room[], target: ClickTarget): JumpDecision {
	const room = rooms.find((r) => r.id === target.roomId);
	if (!room) return { found: false };
	return { found: true, hasHarness: room.harnesses.some((h) => h.id === target.harnessId) };
}

/** Windows only (#155/#294): a live `os-notification-clicked` poke
 * should trigger `os_notify_take_pending` only once `rooms` actually
 * holds the hydrated set (the caller's `loadedRef`, mirroring `loaded`
 * — NOT a "hydrate started" flag, since there's an await gap between
 * the two where `roomsRef` is still empty). Before that, the poke is
 * ignored — the post-hydrate drain (run once `loaded` flips true)
 * already owns picking up the pending target, and draining twice for
 * the same click would just be a harmless no-op second call, but
 * draining *before* rooms are loaded would resolve against an empty
 * list and lose the click. */
export function shouldDrainOnClickEvent(loaded: boolean): boolean {
	return loaded;
}
