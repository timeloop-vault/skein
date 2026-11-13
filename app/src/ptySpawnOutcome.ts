// What the spawn path does with the daemon's `harness_ensure_running` result about
// opencode (#730). The daemon decides v1/v2, starts the sidecar and swaps
// the argv (spawn-time, not stored); the frontend only reads back what
// happened. Pure so it is testable without a DOM or Tauri.

import type { EnsureResult } from "./harnessEnsure.ts";

export interface SpawnOutcome {
	/** The sidecar the daemon started for this PTY, if any (#676). */
	sidecarGeneration: number | undefined;
	/** SSE port for the adapters: none when attaching by sidecar generation
	 *  (#677), the record's pre-allocated port otherwise. */
	port: number | undefined;
}

// #755: the session id and the opencode version the spawn produced are written
// by the daemon itself, so only what the adapters need is read back.
export function spawnOutcome(
	result: Pick<EnsureResult, "sidecarGeneration">,
	record: { opencodePort: number | undefined },
): SpawnOutcome {
	const sidecarGeneration = result.sidecarGeneration ?? undefined;
	return {
		sidecarGeneration,
		port: sidecarGeneration === undefined ? record.opencodePort : undefined,
	};
}
