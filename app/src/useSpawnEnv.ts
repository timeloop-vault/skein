import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import type { SpawnSettings, SpawnSettingsPayload } from "./types.ts";

// Shell / PATH environment (#72, #3, #1). Owned by Rust — the spawn
// path reads it and the shell probe runs during setup(), before this
// webview exists — so this only mirrors it for the Settings UI and
// never feeds it back into a spawn.
export function useSpawnEnv(setDefaultShell: (shell: string[]) => void) {
	const [spawnEnv, setSpawnEnv] = useState<SpawnSettingsPayload | null>(null);
	useEffect(() => {
		void invoke<SpawnSettingsPayload>("spawn_settings_load").then(setSpawnEnv);
	}, []);
	const saveSpawnSettings = useCallback(
		async (next: SpawnSettings) => {
			const payload = await invoke<SpawnSettingsPayload>("spawn_settings_save", {
				settings: next,
			});
			setSpawnEnv(payload);
			// The shell may have changed, and `defaultShell` is what new
			// Shell harnesses and the Enter-for-shell prompt spawn.
			setDefaultShell(await invoke<string[]>("default_shell"));
		},
		[setDefaultShell],
	);
	return { spawnEnv, saveSpawnSettings };
}
