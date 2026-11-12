// What a terminal mount does with `harness_ensure_running`'s result, and what
// it forgets on unmount (#731). Split out of useTerminalSpawn.ts (400-line cap).

import { backgroundTasks } from "./backgroundTasks.ts";
import { claudeVersionStore } from "./claudeVersionStore.ts";
import { harnessActivity } from "./harnessActivity.ts";
import type { EnsureResult } from "./harnessEnsure.ts";
import type { ShellClaimSink } from "./opencodeShellClaim.ts";
import { spawnOutcome } from "./ptySpawnOutcome.ts";
import { shellClaim } from "./shellClaim.ts";
import { forgetSpawnedProfile, recordSpawnedProfile } from "./spawnedEnvProfile.ts";
import { startupAdoption } from "./startupAdoption.ts";
import { subagents } from "./subagents.ts";
import type { HarnessKind } from "./types.ts";

export interface ApplyEnsure {
	result: EnsureResult;
	harnessId: string;
	harnessKind: HarnessKind;
	sessionId: string | undefined;
	opencodePort: number | undefined;
	/** Taken before the call: rows older than this are not the new process's. */
	spawnStartedAt: number;
	opencodeClaim: ShellClaimSink | undefined;
	onSessionCaptured: ((sessionId: string) => void) | undefined;
}

/** Hands the daemon's answer to the view-side stores. Only a process this call
 *  started gets "a fresh process" bookkeeping; a re-attach to a running one
 *  keeps every transcript row as its own (spawnedAt 0). Returns what the
 *  adapters need. */
export function applyEnsureResult(a: ApplyEnsure): {
	sidecarGeneration: number | undefined;
	port: number | undefined;
} {
	const { result, harnessId } = a;
	if (result.started) {
		claudeVersionStore.markSpawned(harnessId, a.spawnStartedAt); // #491
		startupAdoption.forget(harnessId); // #539: a new process voids the old one's candidates
	} else {
		claudeVersionStore.markSpawned(harnessId, 0);
	}
	const outcome = spawnOutcome(result, a.harnessKind, {
		sessionId: a.sessionId,
		opencodePort: a.opencodePort,
	});
	if (outcome.opencodeVersion !== undefined) {
		a.opencodeClaim?.setOpencodeVersion?.(outcome.opencodeVersion);
	}
	if (outcome.capturedSessionId) a.onSessionCaptured?.(outcome.capturedSessionId);
	recordSpawnedProfile(harnessId, result.envProfile ?? null);
	harnessActivity.setInjected(harnessId, result.injected);
	// #677: v2 attaches by sidecar generation, so `port` is undefined there.
	return { sidecarGeneration: outcome.sidecarGeneration, port: outcome.port };
}

/** The per-harness view state dropped on every unmount; a mount re-records it. */
export function forgetViewState(harnessId: string): void {
	harnessActivity.forget(harnessId);
	// #298: Rust rediscovers live subagents on the next attach.
	subagents.forget(harnessId);
	backgroundTasks.forget(harnessId);
	shellClaim.forget(harnessId);
	startupAdoption.forget(harnessId); // #539
	forgetSpawnedProfile(harnessId); // #615
}
