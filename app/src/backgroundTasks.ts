// Pure per-harness tracking of Claude's live background tasks (#446,
// epic #439): a `Bash`/`PowerShell` `run_in_background` command or a
// `Monitor`. A task is not a subagent, but it holds an end of turn the
// same way, so this mirrors `subagents.ts` and feeds the same deferral.
//
// Why the disk stays the source of truth, not this map: the Rust adapter
// re-derives the live set from the transcripts on every attach and
// replays `background_start` for whatever is still live. `record` is
// idempotent for exactly that reason: a replay must read as "still
// going", never as a second start, and must not reset `startedAt`.
//
// A task seeded at attach (`initial === true`) may be an orphan from a
// hard-killed session: a missing `.output` trailer does NOT mean it is
// running. So it is listed but never counted as working (`fromAttach`).
//
// `overdue` is the deferral ceiling's verdict: the task has held an end
// of turn too long. It stays in `live` (it is presumably still running,
// and the UI keeps showing it) but no longer counts as working. Only
// `finish`/`forget`/`expireDue` remove an entry.

import { useSyncExternalStore } from "react";
import type { BackgroundTaskKind } from "./harnessEvents.ts";

/// One live background task, as last reported by `background_start`.
export interface BackgroundTaskEntry {
	taskId: string;
	kind: BackgroundTaskKind;
	description: string | null;
	command: string | null;
	timeoutMs: number | null;
	persistent: boolean;
	/// The launching subagent, or null for the main session.
	agentId: string | null;
	/// Epoch ms first seen. Preserved across a re-record of the same id.
	startedAt: number;
	/// Only ever seen via an attach-time replay. Sticky false: once a
	/// live start is recorded, a later replay must not demote it.
	fromAttach: boolean;
	/// Held an end of turn past the ceiling. Sticky until removed.
	overdue: boolean;
}

export type BackgroundTaskInput = Omit<BackgroundTaskEntry, "startedAt" | "fromAttach" | "overdue">;

const live = new Map<string, Map<string, BackgroundTaskEntry>>();
const listeners = new Map<string, Set<() => void>>();

/// Sorted (oldest-first) snapshot per harness, referentially stable
/// across calls that change nothing, as `useSyncExternalStore` needs.
const snapshots = new Map<string, BackgroundTaskEntry[]>();
const EMPTY: readonly BackgroundTaskEntry[] = [];

const refreshSnapshot = (harnessId: string): void => {
	const forHarness = live.get(harnessId);
	if (!forHarness || forHarness.size === 0) {
		snapshots.delete(harnessId);
		return;
	}
	snapshots.set(
		harnessId,
		[...forHarness.values()].sort((a, b) => a.startedAt - b.startedAt),
	);
};

const emit = (harnessId: string): void => {
	for (const cb of listeners.get(harnessId) ?? []) cb();
};

/// After a mutation: drop an emptied map, rebuild the snapshot, notify.
const commit = (harnessId: string): void => {
	const forHarness = live.get(harnessId);
	if (forHarness && forHarness.size === 0) live.delete(harnessId);
	refreshSnapshot(harnessId);
	emit(harnessId);
};

/// The moment a task ends by its own timeout: only a non-persistent
/// Monitor with a timeout has one. Everything else gets the ceiling.
export function deadlineOf(
	entry: Pick<BackgroundTaskEntry, "kind" | "persistent" | "timeoutMs" | "startedAt">,
): number | null {
	if (entry.kind !== "monitor" || entry.persistent || entry.timeoutMs === null) return null;
	return entry.startedAt + entry.timeoutMs;
}

export const backgroundTasks = {
	/// A task started, or was found still running at attach time.
	/// Idempotent: keeps the original `startedAt` and `overdue`, and
	/// `fromAttach` is sticky false (a live start promotes, a replay
	/// never demotes).
	record(harnessId: string, entry: BackgroundTaskInput, initial: boolean): void {
		let forHarness = live.get(harnessId);
		if (!forHarness) {
			forHarness = new Map();
			live.set(harnessId, forHarness);
		}
		const existing = forHarness.get(entry.taskId);
		forHarness.set(entry.taskId, {
			...entry,
			startedAt: existing?.startedAt ?? Date.now(),
			fromAttach: (existing?.fromAttach ?? true) && initial === true,
			overdue: existing?.overdue ?? false,
		});
		commit(harnessId);
	},

	/// The task ended. No-op for an unknown id.
	finish(harnessId: string, taskId: string): void {
		if (!live.get(harnessId)?.delete(taskId)) return;
		commit(harnessId);
	},

	/// Drop everything tracked for a harness (teardown).
	forget(harnessId: string): void {
		if (!live.delete(harnessId)) return;
		snapshots.delete(harnessId);
		emit(harnessId);
	},

	/// The harness's live tasks, oldest first.
	live(harnessId: string): readonly BackgroundTaskEntry[] {
		return snapshots.get(harnessId) ?? EMPTY;
	},

	/// Tasks that count as working for the deferral: not attach-only
	/// orphans, not overdue.
	workingCount(harnessId: string): number {
		let n = 0;
		for (const e of live.get(harnessId)?.values() ?? []) if (!e.fromAttach && !e.overdue) n++;
		return n;
	},

	/// Tasks the ceiling has marked overdue (attach-only ones excluded).
	overdueCount(harnessId: string): number {
		let n = 0;
		for (const e of live.get(harnessId)?.values() ?? []) if (!e.fromAttach && e.overdue) n++;
		return n;
	},

	/// Remove every entry whose own deadline has passed (a Monitor ends
	/// at its timeout; the Rust sweep only runs on transcript ticks).
	/// Returns how many removed that were not attach-only.
	expireDue(harnessId: string, now: number): number {
		const forHarness = live.get(harnessId);
		if (!forHarness) return 0;
		let removed = 0;
		let changed = false;
		for (const [id, e] of forHarness) {
			const deadline = deadlineOf(e);
			if (deadline === null || deadline > now) continue;
			forHarness.delete(id);
			changed = true;
			if (!e.fromAttach) removed++;
		}
		if (changed) commit(harnessId);
		return removed;
	},

	/// Mark these ids overdue (skipping unknown, attach-only and already
	/// overdue). Returns how many changed.
	markOverdue(harnessId: string, taskIds: readonly string[]): number {
		const forHarness = live.get(harnessId);
		if (!forHarness) return 0;
		let n = 0;
		for (const id of taskIds) {
			const e = forHarness.get(id);
			if (!e || e.fromAttach || e.overdue) continue;
			forHarness.set(id, { ...e, overdue: true });
			n++;
		}
		if (n > 0) commit(harnessId);
		return n;
	},

	/// Stale-work watchdog: mark every working task overdue. Not a
	/// delete, since the task may genuinely still run. Returns the count.
	presumeGone(harnessId: string): number {
		const forHarness = live.get(harnessId);
		if (!forHarness) return 0;
		const ids: string[] = [];
		for (const [id, e] of forHarness) if (!e.fromAttach && !e.overdue) ids.push(id);
		return backgroundTasks.markOverdue(harnessId, ids);
	},

	subscribe(harnessId: string, cb: () => void): () => void {
		let set = listeners.get(harnessId);
		if (!set) {
			set = new Set();
			listeners.set(harnessId, set);
		}
		set.add(cb);
		return () => {
			const s = listeners.get(harnessId);
			if (!s) return;
			s.delete(cb);
			if (s.size === 0) listeners.delete(harnessId);
		};
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
