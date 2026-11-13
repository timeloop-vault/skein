// Per-harness live Claude subagents (#276, epic #298), read side.
//
// Since #732 the daemon's registry is the authority: its `PhaseUpdate`s carry
// the live list and `phaseMirror.ts` holds the latest one. This module is the
// frontend's read-only view of it.

import { useSyncExternalStore } from "react";
import { phaseMirror } from "./phaseMirror.ts";

/// One live subagent, as the daemon reports it.
export interface SubagentEntry {
	agentId: string;
	agentType: string | null;
	description: string | null;
	/// Epoch ms this subagent was first seen.
	startedAt: number;
	/// Only ever seen via an attach-time replay (#277): the PTY that spawned
	/// it is gone, so it is listed but never counts as working.
	fromAttach: boolean;
}

export const subagents = {
	/// The harness's current live subagents, oldest first.
	live(harnessId: string): readonly SubagentEntry[] {
		return phaseMirror.subagents(harnessId);
	},

	/// Subagents that count as "working" for the end-of-turn deferral
	/// (#277): every entry except the attach-time orphans.
	workingCount(harnessId: string): number {
		let n = 0;
		for (const e of phaseMirror.subagents(harnessId)) if (!e.fromAttach) n++;
		return n;
	},

	subscribe(harnessId: string, cb: () => void): () => void {
		return phaseMirror.subscribeSubagents(harnessId, cb);
	},
};

/// React hook: a harness's live-subagent list. Stable reference between
/// renders that don't change this harness's set.
export function useLiveSubagents(harnessId: string): readonly SubagentEntry[] {
	return useSyncExternalStore(
		(cb) => subagents.subscribe(harnessId, cb),
		() => subagents.live(harnessId),
	);
}

/// React hook: `subagents.workingCount` for one harness, kept live.
export function useWorkingSubagentCount(harnessId: string): number {
	return useSyncExternalStore(
		(cb) => subagents.subscribe(harnessId, cb),
		() => subagents.workingCount(harnessId),
	);
}
