// #676: the spawn-time decision between opencode v1 (the harness's stored
// argv, SSE port pinned by Skein) and v2 (`opencode --server <url>` against a
// Skein-owned sidecar). Split out of useTerminalSpawn.ts (400-line cap).
// Invoke and the setters are injected so it is testable without Tauri.

import { invoke } from "@tauri-apps/api/core";
import { opencodeSpawnPlan, opencodeV2Argv } from "./harnessCmd.ts";
import { opencodeCliVersion } from "./opencodeCli.ts";
import type { HarnessKind, OpencodeVersion } from "./types.ts";

export interface SidecarStarted {
	url: string;
	port: number;
	sessionId: string;
	generation: number;
}

export interface SidecarStartArgs {
	roomId: string;
	harnessId: string;
	cwd: string;
	agent: string | null;
	sessionId: string | null;
	envProfile: string | null;
}

export interface PrepareDeps {
	version: () => Promise<OpencodeVersion | null>;
	start: (a: SidecarStartArgs) => Promise<SidecarStarted>;
	/** Stops only that generation, so a newer remount's sidecar survives. */
	stop: (harnessId: string, generation: number) => Promise<void>;
}

export const realPrepareDeps: PrepareDeps = {
	version: opencodeCliVersion,
	start: (a) => invoke<SidecarStarted>("opencode_sidecar_start", { ...a }),
	stop: (harnessId, generation) => invoke<void>("opencode_sidecar_stop", { harnessId, generation }),
};

export interface PrepareOpts {
	kind: HarnessKind;
	/** The harness's stored argv. */
	cmd: string[];
	roomId: string;
	harnessId: string;
	cwd: string;
	/** From the harness RECORD, never parsed from the argv. */
	sessionId: string | undefined;
	agent: string | undefined;
	envProfile: string | undefined;
	setOpencodeVersion: ((version: OpencodeVersion | null) => void) | undefined;
	/** The existing capture setter; the harness record gets the session id. */
	setSessionId: ((sessionId: string) => void) | undefined;
	isCancelled: () => boolean;
	deps?: PrepareDeps;
}

export interface PreparedSpawn {
	argv: string[];
	v2: boolean;
	sidecarGeneration?: number;
}

/** Resolve the argv to spawn. Resolves null when cancelled (a sidecar
 *  already started is stopped first). Rejects when the sidecar fails. */
export async function prepareOpencodeSpawn(o: PrepareOpts): Promise<PreparedSpawn | null> {
	if (o.kind !== "opencode") return { argv: o.cmd, v2: false };
	const deps = o.deps ?? realPrepareDeps;
	const version = await deps.version();
	if (o.isCancelled()) return null;
	o.setOpencodeVersion?.(version);
	if (opencodeSpawnPlan(o.cmd, o.kind, version).mode === "v1") return { argv: o.cmd, v2: false };
	const started = await deps.start({
		roomId: o.roomId,
		harnessId: o.harnessId,
		cwd: o.cwd,
		agent: o.agent?.trim() ? o.agent : null,
		sessionId: o.sessionId?.trim() ? o.sessionId : null,
		envProfile: o.envProfile ?? null,
	});
	if (o.isCancelled()) {
		void deps.stop(o.harnessId, started.generation).catch(() => {});
		return null;
	}
	if (started.sessionId !== o.sessionId) o.setSessionId?.(started.sessionId);
	return {
		argv: opencodeV2Argv(started.url, started.sessionId),
		v2: true,
		sidecarGeneration: started.generation,
	};
}
