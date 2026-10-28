// Mirror of SpawnSettings.contextThreshold (#368), fed by
// `publishSpawnSettings` whenever the settings are loaded or saved. Rust owns
// the value; this only lets hooks subscribe to it. null = feature off.

import { useSyncExternalStore } from "react";

let threshold: number | null = null;
const listeners = new Set<() => void>();

export function setContextThreshold(value: number | null | undefined): void {
	const next = value ?? null;
	if (next === threshold) return;
	threshold = next;
	for (const l of listeners) l();
}

export function getContextThreshold(): number | null {
	return threshold;
}

export function subscribeContextThreshold(cb: () => void): () => void {
	listeners.add(cb);
	return () => {
		listeners.delete(cb);
	};
}

export function useContextThreshold(): number | null {
	return useSyncExternalStore(subscribeContextThreshold, getContextThreshold);
}
