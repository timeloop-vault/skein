// React hooks over the harness activity store (epic #50). Split out of
// harnessActivity.ts (#19) — these read the single store instance living
// in harnessActivityCore.ts and the `harnessActivity` object in
// harnessActivity.ts. See harnessActivity.ts for the module this belongs
// to.

import { useCallback, useSyncExternalStore } from "react";
import { harnessActivity } from "./harnessActivity.ts";
import {
	store as activityStore,
	permissionIds,
	permissionListeners,
} from "./harnessActivityCore.ts";
import { aggregateRoomStatus } from "./harnessActivityLabels.ts";
import type { RoomHarnessRef } from "./harnessActivityLabels.ts";
import type { HarnessActivity } from "./harnessActivityTypes.ts";
import type { Status } from "./types.ts";

/// React hook: subscribes to the activity store for a single
/// harness and returns its current state. Returns `null` for
/// unknown ids (e.g. archived harnesses) so callers can fall
/// back gracefully.
export function useHarnessActivity(id: string | null): HarnessActivity | null {
	return useSyncExternalStore(
		(cb) => {
			if (id === null) return () => {};
			return harnessActivity.subscribe(id, cb);
		},
		() => (id === null ? null : harnessActivity.get(id)),
	);
}

/// React hook: every harness id currently in `permission`, across
/// every room. Backs the status-bar urgent slot (#86), which needs to
/// know whether *anything* is blocked on a dialog without subscribing
/// to each harness in every room individually. The returned Set is
/// referentially stable across renders that don't change membership,
/// as `useSyncExternalStore` requires.
export function usePermissionHarnessIds(): ReadonlySet<string> {
	return useSyncExternalStore(
		(cb) => {
			permissionListeners.add(cb);
			return () => {
				permissionListeners.delete(cb);
			};
		},
		() => permissionIds,
	);
}

/// React hook: subscribes to every harness in a room and returns
/// the aggregate room status via `aggregateRoomStatus`. Always
/// returns a concrete `Status` — `"idle"` when no harness in the
/// list has a record yet (#290).
///
/// Takes the full harness records (rather than just ids) so the
/// aggregation can apply the same acknowledged-downgrade rule
/// `effectiveStatus` uses per-harness — a room dot shouldn't pulse
/// for a harness the user has already seen.
///
/// Caller responsibility: pass a stable `harnesses` reference
/// (use useMemo). `useSyncExternalStore` re-subscribes whenever
/// `subscribe` changes; a fresh array reference each render would
/// thrash the listener Sets without changing behaviour.
export function useRoomActivity(harnesses: readonly RoomHarnessRef[]): Status {
	const subscribe = useCallback(
		(cb: () => void) => {
			const unsubs = harnesses.map((h) => harnessActivity.subscribe(h.id, cb));
			return () => {
				for (const u of unsubs) u();
			};
		},
		[harnesses],
	);
	const getSnapshot = useCallback(
		() => aggregateRoomStatus(harnesses, (id) => activityStore.get(id)),
		[harnesses],
	);
	return useSyncExternalStore(subscribe, getSnapshot);
}
