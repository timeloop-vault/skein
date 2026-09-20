// Pure per-harness tracking of Claude's live subagents (#276, epic
// #298). A subagent runs in its own transcript, invisible to the main
// JSONL tail — this module is the frontend's only record of which
// subagents are currently running under a harness, and it exists so
// #277 (subagent-aware phase/notification policy) has something clean
// to read.
//
// Why the disk stays the source of truth, not this map: the Rust side
// rediscovers every still-running subagent from its own transcript the
// moment it (re)attaches — on a fresh `subagent_start` and again after
// a Skein restart, for every subagent that hasn't reached a terminal
// stop reason yet. `record` is idempotent for exactly that reason: a
// restart replays `subagent_start` for everything still live, and that
// replay must not look like a second subagent starting, nor reset how
// long it's been running. That rediscovery is also what guarantees
// this map can never leak a permanently-stuck entry the way a
// forget-on-detach scheme could — the next attach simply reports the
// truth again.
//
// Kept separate from `harnessActivity` so that membership logic is
// unit-testable without the store, the event stream, or React — same
// split `pendingPrompts` makes for opencode's prompt bookkeeping.

import { useSyncExternalStore } from "react";

/// One live subagent, as last reported by `subagent_start`.
export interface SubagentEntry {
	agentId: string;
	agentType: string | null;
	description: string | null;
	/// Epoch ms this subagent was first seen. Preserved across a
	/// redundant `subagent_start` for the same `agentId` — see
	/// `record`.
	startedAt: number;
}

const live = new Map<string, Map<string, SubagentEntry>>();
const listeners = new Map<string, Set<() => void>>();

/// Sorted (oldest-first) snapshot per harness, kept referentially
/// stable across calls that don't change membership — required by
/// `useSyncExternalStore`, which re-renders whenever `getSnapshot`
/// returns a new reference. Rebuilt, not incrementally maintained,
/// on every mutation; the set is small enough that a full sort is
/// cheap, same tradeoff `harnessActivity` makes for `permissionIds`.
const snapshots = new Map<string, SubagentEntry[]>();
const EMPTY: readonly SubagentEntry[] = [];

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

export const subagents = {
	/// A subagent transcript appeared, or one was found still running
	/// at attach time. Idempotent: a second `subagent_start` for an
	/// `agentId` already tracked keeps the original `startedAt` and
	/// does not double-count — the Rust side re-emits this after every
	/// restart for whatever is still live, and that replay must read
	/// as "still going," not "started again."
	record(
		harnessId: string,
		entry: { agentId: string; agentType: string | null; description: string | null },
	): void {
		let forHarness = live.get(harnessId);
		if (!forHarness) {
			forHarness = new Map();
			live.set(harnessId, forHarness);
		}
		const existing = forHarness.get(entry.agentId);
		forHarness.set(entry.agentId, { ...entry, startedAt: existing?.startedAt ?? Date.now() });
		refreshSnapshot(harnessId);
		emit(harnessId);
	},

	/// The subagent reached a terminal stop reason. No-op for an
	/// unknown `agentId` (already finished, or never tracked).
	finish(harnessId: string, agentId: string): void {
		const forHarness = live.get(harnessId);
		if (!forHarness?.delete(agentId)) return;
		if (forHarness.size === 0) live.delete(harnessId);
		refreshSnapshot(harnessId);
		emit(harnessId);
	},

	/// Drop every subagent tracked for a harness. Wired into the same
	/// teardown path that calls `harnessActivity.forget` — otherwise
	/// this map grows for the life of the app.
	forget(harnessId: string): void {
		if (!live.delete(harnessId)) return;
		snapshots.delete(harnessId);
		emit(harnessId);
	},

	/// The harness's current live subagents, oldest first.
	live(harnessId: string): readonly SubagentEntry[] {
		return snapshots.get(harnessId) ?? EMPTY;
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

/// React hook: subscribes to a harness's live-subagent list. Stable
/// reference between renders that don't change this harness's set.
export function useLiveSubagents(harnessId: string): readonly SubagentEntry[] {
	return useSyncExternalStore(
		(cb) => subagents.subscribe(harnessId, cb),
		() => subagents.live(harnessId),
	);
}
