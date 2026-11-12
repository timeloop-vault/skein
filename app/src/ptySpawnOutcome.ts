// What the spawn path does with the daemon's `harness_ensure_running` result about
// opencode (#730). The daemon decides v1/v2, starts the sidecar and swaps
// the argv (spawn-time, not stored); the frontend only reads back what
// happened. Pure so it is testable without a DOM or Tauri.

import type { EnsureResult } from "./harnessEnsure.ts";
import type { HarnessKind, OpencodeVersion } from "./types.ts";

export interface SpawnOutcome {
	/** The sidecar the daemon started for this PTY, if any (#676). */
	sidecarGeneration: number | undefined;
	/** SSE port for the adapters: none when attaching by sidecar generation
	 *  (#677), the record's pre-allocated port otherwise. */
	port: number | undefined;
	/** The CLI version the daemon probed, to hand to the claim sink. `null` =
	 *  an opencode spawn whose probe found nothing: the setter must still be
	 *  called with it, because null DELETES the stored version (a harness
	 *  recorded as v2 must not keep reading as v2 once the CLI is gone).
	 *  `undefined` = not opencode, nothing was probed. */
	opencodeVersion: OpencodeVersion | null | undefined;
	/** A session id the daemon created or confirmed that differs from the
	 *  record's: to be written back through `onSessionCaptured`. */
	capturedSessionId: string | undefined;
}

export function spawnOutcome(
	result: Pick<EnsureResult, "sidecarGeneration" | "opencodeVersion" | "sessionId">,
	kind: HarnessKind,
	record: { sessionId: string | undefined; opencodePort: number | undefined },
): SpawnOutcome {
	const sidecarGeneration = result.sidecarGeneration ?? undefined;
	const sid = result.sessionId ?? undefined;
	return {
		sidecarGeneration,
		port: sidecarGeneration === undefined ? record.opencodePort : undefined,
		opencodeVersion: kind === "opencode" ? (result.opencodeVersion ?? null) : undefined,
		capturedSessionId: sid !== undefined && sid !== record.sessionId ? sid : undefined,
	};
}
