// Module-level store for the Claude Code version notice (#491), same
// subscribe + useSyncExternalStore shape as `mailHold.ts`. One global
// "installed" version, plus per-harness running version / spawn time.

import { useMemo, useSyncExternalStore } from "react";
import { type VersionNotice, versionNotice } from "./claudeVersion.ts";

interface Entry {
	spawnedAt: number | null;
	running: string | null;
	refusal: string | null;
	autoRestartedFor: string | null;
}

const entries = new Map<string, Entry>();
let installedVersion: string | null = null;
let version = 0;
const listeners = new Set<() => void>();

const emit = (): void => {
	version++;
	for (const cb of [...listeners]) cb();
};

const entry = (id: string): Entry => {
	let e = entries.get(id);
	if (!e) {
		e = { spawnedAt: null, running: null, refusal: null, autoRestartedFor: null };
		entries.set(id, e);
	}
	return e;
};

export const claudeVersionStore = {
	/// A new process: forget what the old one ran. autoRestartedFor stays,
	/// it is per installed version.
	markSpawned(harnessId: string, atMs: number): void {
		const e = entry(harnessId);
		e.spawnedAt = atMs;
		e.running = null;
		e.refusal = null;
		emit();
	},

	/// Rows written before this process started say nothing about it.
	recordRunning(harnessId: string, ver: string, timestampMs: number | null): void {
		const e = entries.get(harnessId);
		if (!e || e.spawnedAt === null || timestampMs === null) return;
		if (timestampMs < e.spawnedAt) return;
		if (e.running === ver) return;
		e.running = ver;
		emit();
	},

	setInstalled(ver: string | null): void {
		if (installedVersion === ver) return;
		installedVersion = ver;
		emit();
	},

	installed(): string | null {
		return installedVersion;
	},

	markAutoRestarted(harnessId: string, installedVer: string): void {
		entry(harnessId).autoRestartedFor = installedVer;
		emit();
	},

	clearAutoRestarted(harnessId: string): void {
		const e = entries.get(harnessId);
		if (!e || e.autoRestartedFor === null) return;
		e.autoRestartedFor = null;
		emit();
	},

	autoRestartedFor(harnessId: string): string | null {
		return entries.get(harnessId)?.autoRestartedFor ?? null;
	},

	setRefusal(harnessId: string, reason: string | null): void {
		const e = entry(harnessId);
		if (e.refusal === reason) return;
		e.refusal = reason;
		emit();
	},

	refusal(harnessId: string): string | null {
		return entries.get(harnessId)?.refusal ?? null;
	},

	forget(harnessId: string): void {
		if (!entries.delete(harnessId)) return;
		emit();
	},

	notice(harnessId: string): VersionNotice | null {
		return versionNotice(entries.get(harnessId)?.running, installedVersion);
	},

	subscribe(cb: () => void): () => void {
		listeners.add(cb);
		return () => {
			listeners.delete(cb);
		};
	},

	/// Test helper: drop everything.
	reset(): void {
		entries.clear();
		installedVersion = null;
		emit();
	},
};

export function useVersionNotice(harnessId: string): {
	notice: VersionNotice | null;
	refusal: string | null;
} {
	const v = useSyncExternalStore(
		claudeVersionStore.subscribe,
		() => version,
		() => version,
	);
	// biome-ignore lint/correctness/useExhaustiveDependencies: v is the change signal
	return useMemo(
		() => ({
			notice: claudeVersionStore.notice(harnessId),
			refusal: claudeVersionStore.refusal(harnessId),
		}),
		[harnessId, v],
	);
}
