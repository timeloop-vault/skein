// Mirror of SpawnSettings.contextNudge (#689), fed by `publishSpawnSettings`
// whenever the settings are loaded or saved. Rust owns the value; this only
// lets components and the Shell & environment save read the latest copy.

import { useSyncExternalStore } from "react";
import { type ContextNudgeSettings, DEFAULT_CONTEXT_NUDGE } from "./contextNudge.ts";

let current: ContextNudgeSettings = DEFAULT_CONTEXT_NUDGE;
const listeners = new Set<() => void>();

export function setContextNudge(value: ContextNudgeSettings | null | undefined): void {
	const next = value ?? DEFAULT_CONTEXT_NUDGE;
	if (next.enabled === current.enabled && next.body === current.body) return;
	current = next;
	for (const l of listeners) l();
}

export function getContextNudge(): ContextNudgeSettings {
	return current;
}

function subscribe(cb: () => void): () => void {
	listeners.add(cb);
	return () => {
		listeners.delete(cb);
	};
}

export function useContextNudge(): ContextNudgeSettings {
	return useSyncExternalStore(subscribe, getContextNudge);
}
