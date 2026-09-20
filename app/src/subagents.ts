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
	/// Has this id only ever been seen via an attach-time replay
	/// (`ClaudeEvent.initial === true`), never a live start? `true`
	/// means the transcript was already on disk when the adapter
	/// attached — the PTY that spawned it is gone (PTYs die with
	/// Skein), so on real transcripts it's typically days-old and
	/// nothing is coming. #277 (epic #298): 2.8% of real subagent
	/// transcripts are exactly these orphans, and counting one as
	/// "working" would suppress a harness's notifications for the
	/// whole delegation-ceiling window after every restart — worse
	/// than the missed notification it would be covering for. Sticky
	/// false: once a live start is recorded for an id, a later replay
	/// (Skein restarting while it's still genuinely running) must not
	/// revert it — see `record`.
	fromAttach: boolean;
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
	///
	/// `initial` mirrors `ClaudeEvent.initial` (#277): `true` for the
	/// attach-time batch found already on disk, `false` for a start
	/// observed live. `fromAttach` is derived rather than copied
	/// straight across so a later live re-emit for an id first seen at
	/// attach promotes it — once `false`, always `false`.
	record(
		harnessId: string,
		entry: { agentId: string; agentType: string | null; description: string | null },
		initial: boolean,
	): void {
		let forHarness = live.get(harnessId);
		if (!forHarness) {
			forHarness = new Map();
			live.set(harnessId, forHarness);
		}
		const existing = forHarness.get(entry.agentId);
		forHarness.set(entry.agentId, {
			...entry,
			startedAt: existing?.startedAt ?? Date.now(),
			fromAttach: (existing?.fromAttach ?? true) && initial === true,
		});
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

	/// Count of this harness's subagents that count as "working" for
	/// #277's deferral (Rules 1/3/4) — every tracked entry EXCEPT the
	/// attach-time-only orphans `fromAttach` exists to exclude. `live`
	/// above stays an honest mirror of the transcripts (#276's
	/// contract); this is the one place the counting is opinionated.
	workingCount(harnessId: string): number {
		const forHarness = live.get(harnessId);
		if (!forHarness) return 0;
		let n = 0;
		for (const e of forHarness.values()) if (!e.fromAttach) n++;
		return n;
	},

	/// #277's ceiling: presume every "working" subagent for this
	/// harness is gone — no event from any of them in
	/// `DELEGATION_CEILING_MS`, so a signal was almost certainly lost.
	/// Leaves attach-time-only entries alone; they were never counted
	/// as working, so there's nothing to presume about them, and the
	/// disk stays the eventual source of truth if one really is still
	/// running (its next event re-records it). Returns how many were
	/// dropped so the caller can name the count in its warning.
	presumeGone(harnessId: string): number {
		const forHarness = live.get(harnessId);
		if (!forHarness) return 0;
		let n = 0;
		for (const [id, e] of forHarness) {
			if (e.fromAttach) continue;
			forHarness.delete(id);
			n++;
		}
		if (n === 0) return 0;
		if (forHarness.size === 0) live.delete(harnessId);
		refreshSnapshot(harnessId);
		emit(harnessId);
		return n;
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

/// React hook: `subagents.workingCount` for one harness, kept live.
/// #277's `statusLabel` call sites need this reactively — a number is
/// already a stable primitive across renders that don't change it, so
/// unlike `useLiveSubagents` there's no snapshot-caching to do.
export function useWorkingSubagentCount(harnessId: string): number {
	return useSyncExternalStore(
		(cb) => subagents.subscribe(harnessId, cb),
		() => subagents.workingCount(harnessId),
	);
}
