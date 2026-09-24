// Reactive wrapper over the persisted nudge-body overrides (#355),
// mirroring the tiny-store + `useSyncExternalStore` shape `mailStore.ts`
// and `subagents.ts` already use — a plain module-level value plus a
// listener set, no React state.
//
// `nudgeRegistry.ts` stays pure and React-free on purpose (it is what
// `review/nudges.ts` imports, and that module is tested with no DOM);
// this is the seam that makes an edit in a future Settings UI show up
// in `ReviewPane` without a page reload. `prefs.ts` is the actual
// localStorage read/write — this module just keeps one in-memory copy
// in sync with it and notifies subscribers on change.

import { useSyncExternalStore } from "react";
import type { NudgeOverrides } from "./nudgeRegistry.ts";
import { withOverride, withoutOverride } from "./nudgeRegistry.ts";
import { loadNudgeOverrides, saveNudgeOverrides } from "./prefs.ts";

let overrides: NudgeOverrides = loadNudgeOverrides();
const listeners = new Set<() => void>();

const emit = (): void => {
	for (const cb of listeners) cb();
};

const commit = (next: NudgeOverrides): void => {
	if (next === overrides) return;
	overrides = next;
	saveNudgeOverrides(overrides);
	emit();
};

/// The current override map. Read-through cache over `prefs.ts` —
/// loaded once at module init, kept current by `setNudgeBody` /
/// `resetNudgeBody`, since nothing else in this app writes that
/// localStorage key.
export function getNudgeOverrides(): NudgeOverrides {
	return overrides;
}

/// Set (or, per `withOverride`'s own blank/default-equal rule, clear)
/// the override for `id` and persist it.
export function setNudgeBody(id: string, body: string): void {
	commit(withOverride(overrides, id, body));
}

/// Reset `id` back to its registry default and persist it.
export function resetNudgeBody(id: string): void {
	commit(withoutOverride(overrides, id));
}

export function subscribe(cb: () => void): () => void {
	listeners.add(cb);
	return () => {
		listeners.delete(cb);
	};
}

/// React hook: the live override map, re-rendering on every edit or
/// reset from anywhere in the app.
export function useNudgeOverrides(): NudgeOverrides {
	return useSyncExternalStore(subscribe, getNudgeOverrides);
}
