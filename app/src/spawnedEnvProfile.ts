// Which env profile each harness's live PTY was actually spawned with
// (#615). The value comes from `pty_spawn`'s result, so it is what Rust
// really applied, not what the UI believes. Compared against the profile
// that WOULD apply now to tell the user a restart is needed.
//
// In-memory only, like `subagents.ts`: PTYs die with Skein, so there is
// nothing to persist. `undefined` = no spawn recorded yet (nothing to be
// stale against); `null` = spawned with no profile.

import { useSyncExternalStore } from "react";

const spawned = new Map<string, string | null>();
const listeners = new Set<() => void>();

function emit(): void {
	for (const l of listeners) l();
}

export function recordSpawnedProfile(harnessId: string, name: string | null): void {
	if (spawned.has(harnessId) && spawned.get(harnessId) === name) return;
	spawned.set(harnessId, name);
	emit();
}

export function spawnedProfile(harnessId: string): string | null | undefined {
	return spawned.get(harnessId);
}

export function forgetSpawnedProfile(harnessId: string): void {
	if (spawned.delete(harnessId)) emit();
}

function subscribe(cb: () => void): () => void {
	listeners.add(cb);
	return () => {
		listeners.delete(cb);
	};
}

export function useSpawnedProfile(harnessId: string): string | null | undefined {
	return useSyncExternalStore(
		subscribe,
		() => spawned.get(harnessId),
		() => undefined,
	);
}

/** True only when a spawn was recorded and it differs from the profile
 *  that would apply now. */
export function profileIsStale(spawnedName: string | null | undefined, effective: string | null) {
	return spawnedName !== undefined && spawnedName !== effective;
}

/** Mirrors Rust's resolution: the harness override if it names a known
 *  profile, else the global active one if known, else none. */
export function effectiveProfile(
	harnessOverride: string | undefined,
	active: string | null | undefined,
	known: string[],
): string | null {
	// Rust trims names when resolving; match it on both sides.
	const names = known.map((n) => n.trim());
	const over = harnessOverride?.trim();
	if (over !== undefined && names.includes(over)) return over;
	const act = active?.trim();
	if (act !== undefined && act !== "" && names.includes(act)) return act;
	return null;
}
