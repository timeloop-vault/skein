// Per-harness live Claude background tasks (#446, epic #439), read side.
//
// A `Bash`/`PowerShell` `run_in_background` command or a `Monitor` is a task,
// not a subagent, but it holds an end of turn the same way. Since #732 the
// daemon's registry is the authority and `phaseMirror.ts` holds its latest
// list; this module is the frontend's read-only view of it.

import { useSyncExternalStore } from "react";
import type { BackgroundTaskKind } from "./harnessEvents.ts";
import { phaseMirror } from "./phaseMirror.ts";

/// One live background task, as the daemon reports it.
export interface BackgroundTaskEntry {
	taskId: string;
	kind: BackgroundTaskKind;
	description: string | null;
	command: string | null;
	timeoutMs: number | null;
	persistent: boolean;
	/// The launching subagent, or null for the main session.
	agentId: string | null;
	/// Epoch ms first seen.
	startedAt: number;
	/// Only ever seen via an attach-time replay: listed, never working.
	fromAttach: boolean;
	/// Held an end of turn past the ceiling: listed, no longer working.
	overdue: boolean;
	/// Bookkeeping the daemon owns; never affects display.
	overdueReported: boolean;
}

/// The moment a task ends by its own timeout: only a non-persistent
/// Monitor with a timeout has one. Everything else gets the ceiling.
export function deadlineOf(
	entry: Pick<BackgroundTaskEntry, "kind" | "persistent" | "timeoutMs" | "startedAt">,
): number | null {
	if (entry.kind !== "monitor" || entry.persistent || entry.timeoutMs === null) return null;
	return entry.startedAt + entry.timeoutMs;
}

export const backgroundTasks = {
	/// The harness's live tasks, oldest first.
	live(harnessId: string): readonly BackgroundTaskEntry[] {
		return phaseMirror.backgroundTasks(harnessId);
	},

	/// Tasks that count as working for the deferral: not attach-only
	/// orphans, not overdue.
	workingCount(harnessId: string): number {
		let n = 0;
		for (const e of phaseMirror.backgroundTasks(harnessId)) if (!e.fromAttach && !e.overdue) n++;
		return n;
	},

	/// Overdue tasks (attach-only excluded) no earlier ceiling notice has
	/// announced; marks them announced. Silent.
	takeUnreportedOverdue(harnessId: string): number {
		return phaseMirror.takeUnreportedOverdue(harnessId);
	},

	subscribe(harnessId: string, cb: () => void): () => void {
		return phaseMirror.subscribeTasks(harnessId, cb);
	},
};

/// React hook: a harness's live background tasks, referentially stable.
export function useLiveBackgroundTasks(harnessId: string): readonly BackgroundTaskEntry[] {
	return useSyncExternalStore(
		(cb) => backgroundTasks.subscribe(harnessId, cb),
		() => backgroundTasks.live(harnessId),
	);
}

/// React hook: `backgroundTasks.workingCount` for one harness, kept live.
export function useWorkingBackgroundTaskCount(harnessId: string): number {
	return useSyncExternalStore(
		(cb) => backgroundTasks.subscribe(harnessId, cb),
		() => backgroundTasks.workingCount(harnessId),
	);
}
