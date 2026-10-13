// App-level mirror of the env-profile selection (#615), so the status bar,
// command palette and Settings agree. Rust owns the settings file; this only
// holds the profile names + the active one, refreshed whenever the settings
// are loaded or saved from anywhere in the UI.

import { invoke } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";
import {
	profileNames,
	type Renames,
	recordRename,
	resolveRenamed,
	withActiveProfile,
} from "./envProfiles.ts";
import type { SpawnSettings, SpawnSettingsPayload } from "./types.ts";

export interface EnvProfileState {
	names: string[];
	active: string | null;
	/** False until the first settings load/save has been published. */
	loaded: boolean;
}

const EMPTY: EnvProfileState = { names: [], active: null, loaded: false };

/** Pure: what the store keeps from a settings blob. An active name that no
 *  longer exists reads as none. */
export function stateFromSettings(
	settings: Pick<SpawnSettings, "envProfiles" | "activeEnvProfile"> | null | undefined,
): EnvProfileState {
	if (!settings) return EMPTY;
	const names = profileNames(settings);
	const active = settings.activeEnvProfile ?? null;
	return {
		names,
		active: active !== null && names.includes(active) ? active : null,
		loaded: true,
	};
}

export function sameState(a: EnvProfileState, b: EnvProfileState): boolean {
	return (
		a.active === b.active &&
		a.loaded === b.loaded &&
		a.names.length === b.names.length &&
		a.names.every((n, i) => n === b.names[i])
	);
}

let state: EnvProfileState = EMPTY;
const listeners = new Set<() => void>();

/** Feed the store from any successful settings load or save. */
export function publishSpawnSettings(
	settings: Pick<SpawnSettings, "envProfiles" | "activeEnvProfile"> | null | undefined,
): void {
	const next = stateFromSettings(settings);
	if (sameState(state, next)) return;
	state = next;
	for (const l of listeners) l();
}

export function envProfileState(): EnvProfileState {
	return state;
}

/** Switch the global active profile. Loads the current settings fresh so no
 *  other setting is clobbered by a stale copy. */
export function setActiveEnvProfile(name: string | null): Promise<void> {
	return queueSettingsWrite(() => applyActive(name));
}

// One chain for every settings write (chip switches and the Settings save),
// so a load-then-save pair can't interleave with another writer and write a
// stale profile list back. `fn` must not itself queue another write.
let writeChain: Promise<unknown> = Promise.resolve();

export function queueSettingsWrite<T>(fn: () => Promise<T>): Promise<T> {
	const run = writeChain.then(fn);
	writeChain = run.catch(() => undefined);
	return run;
}

// Renames applied by saves this session (old -> current), so a switch
// captured before a rename was saved still lands on the renamed profile.
let savedRenames: Renames = {};
const renameListeners = new Set<(renames: Renames) => void>();

/** Remember renames a successful save applied and tell the listeners (App
 *  maps per-harness overrides). Call it from inside the write chain. */
export function recordSavedRenames(renames: Renames): void {
	const entries = Object.entries(renames);
	if (entries.length === 0) return;
	for (const [from, to] of entries) savedRenames = recordRename(savedRenames, from, to);
	for (const l of renameListeners) l(renames);
}

export function onProfilesRenamed(cb: (renames: Renames) => void): () => void {
	renameListeners.add(cb);
	return () => {
		renameListeners.delete(cb);
	};
}

async function applyActive(name: string | null): Promise<void> {
	const current = await invoke<SpawnSettingsPayload>("spawn_settings_load");
	const target = resolveRenamed(savedRenames, name, profileNames(current.settings));
	const saved = await invoke<SpawnSettingsPayload>("spawn_settings_save", {
		settings: withActiveProfile(current.settings, target),
	});
	publishSpawnSettings(saved.settings);
}

function subscribe(cb: () => void): () => void {
	listeners.add(cb);
	return () => {
		listeners.delete(cb);
	};
}

export function useEnvProfiles(): EnvProfileState & {
	setActive: (name: string | null) => Promise<void>;
} {
	const s = useSyncExternalStore(subscribe, envProfileState, envProfileState);
	return { ...s, setActive: setActiveEnvProfile };
}
