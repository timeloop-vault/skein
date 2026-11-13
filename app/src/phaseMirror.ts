// The webview's mirror of the daemon's harness phase machine (#732 part 2).
//
// The authority is the daemon's Rust `PhaseService`; it publishes a
// `PhaseUpdate` on `skein://harness-phase` for every transition, for a #277
// delegation deferral arming, and at most once a second for anything else.
// This module keeps the latest update per harness, notifies listeners in
// the order the old in-process machine did, and decides nothing itself.
//
// The core (`applyUpdate`, `applySnapshot` and the stores) imports nothing
// from Tauri's runtime, so it is testable with plain objects. `startPhaseMirror`
// is the thin boundary: listen, buffer, fetch the snapshot, reconcile.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { BackgroundTaskEntry } from "./backgroundTasks.ts";
import { logToRust } from "./frontendLog.ts";
import type { ActivityPhase, HarnessActivity, TransitionSource } from "./harnessActivityTypes.ts";
import type { SubagentEntry } from "./subagents.ts";

export const HARNESS_PHASE_EVENT = "skein://harness-phase";

/// One phase change the daemon reports in an update, oldest first.
export interface PhaseTransition {
	harnessId: string;
	from: ActivityPhase;
	to: ActivityPhase;
	source: TransitionSource;
	atMs: number;
	hasUserInput: boolean;
}

/// Mirror of `PhaseUpdate` in skein-daemon `harness_phase/stream.rs`.
export interface PhaseUpdate {
	seq: number;
	harnessId: string;
	roomId: string;
	/// `null`: the harness was forgotten.
	view: HarnessActivity | null;
	subagents: SubagentEntry[];
	backgroundTasks: BackgroundTaskEntry[];
	transitions: PhaseTransition[];
	delegationArmed: boolean;
}

type TransitionCb = (
	id: string,
	from: ActivityPhase,
	to: ActivityPhase,
	source: TransitionSource,
) => void;

const records = new Map<string, HarnessActivity>();
const subagentLists = new Map<string, readonly SubagentEntry[]>();
const taskLists = new Map<string, readonly BackgroundTaskEntry[]>();
const lastSeq = new Map<string, number>();
/// `harnessId` -> task ids a ceiling notice has already announced. Client
/// side bookkeeping only; the daemon never sets `overdueReported`.
const announcedOverdue = new Map<string, Set<string>>();

const harnessListeners = new Map<string, Set<() => void>>();
const subagentListeners = new Map<string, Set<() => void>>();
const taskListeners = new Map<string, Set<() => void>>();
const transitionListeners = new Set<TransitionCb>();
const deferralListeners = new Set<(id: string) => void>();
const permissionListeners = new Set<() => void>();

const EMPTY_SUBAGENTS: readonly SubagentEntry[] = [];
const EMPTY_TASKS: readonly BackgroundTaskEntry[] = [];
let permissionIds: ReadonlySet<string> = new Set();

function safely(fn: () => void): void {
	try {
		fn();
	} catch (e) {
		console.error("[skein] phase mirror listener threw", e);
	}
}

function fire(map: Map<string, Set<() => void>>, id: string): void {
	for (const cb of [...(map.get(id) ?? [])]) safely(cb);
}

function addTo<K, V>(map: Map<K, Set<V>>, key: K, cb: V): () => void {
	let set = map.get(key);
	if (!set) {
		set = new Set();
		map.set(key, set);
	}
	set.add(cb);
	return () => {
		const s = map.get(key);
		if (!s) return;
		s.delete(cb);
		if (s.size === 0) map.delete(key);
	};
}

const sameJson = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);

/// Everything but `lastOutputAt` equal.
function sameApartFromOutput(a: HarnessActivity, b: HarnessActivity): boolean {
	return sameJson({ ...a, lastOutputAt: null }, { ...b, lastOutputAt: null });
}

function recomputePermissionIds(): void {
	const next = new Set<string>();
	for (const [id, a] of records) if (a.phase === "permission") next.add(id);
	if (next.size === permissionIds.size && [...next].every((id) => permissionIds.has(id))) return;
	permissionIds = next;
	for (const cb of [...permissionListeners]) safely(cb);
}

function dispatchEvents(u: PhaseUpdate): void {
	for (const t of u.transitions) {
		for (const cb of [...transitionListeners])
			safely(() => cb(t.harnessId, t.from, t.to, t.source));
	}
	if (u.delegationArmed) {
		for (const cb of [...deferralListeners]) safely(() => cb(u.harnessId));
	}
}

/// Apply one update from the stream or a snapshot. Returns false when its
/// state was stale (`seq` not above the last applied for the harness).
export function applyUpdate(u: PhaseUpdate): boolean {
	const id = u.harnessId;
	const seen = lastSeq.get(id);
	if (seen !== undefined && u.seq <= seen) {
		// Built before a snapshot that already holds its state, but delivered
		// after it (or buffered at start-up). The record and lists stay as the
		// snapshot left them, yet its transitions were never announced: the
		// snapshot carries none. The protocol is one ordered stream with no
		// redelivery, so each transition arrives here at most once and this
		// cannot fire twice.
		dispatchEvents(u);
		return false;
	}
	lastSeq.set(id, u.seq);

	const prev = records.get(id);
	let changed = false;
	if (u.view === null) {
		changed = records.delete(id);
		announcedOverdue.delete(id);
	} else {
		const next: HarnessActivity = { ...u.view };
		// The daemon throttles `lastOutputAt`; the local stamp may be newer.
		if (prev && prev.spawnedAt === next.spawnedAt && prev.lastOutputAt !== null) {
			next.lastOutputAt = Math.max(prev.lastOutputAt, next.lastOutputAt ?? 0);
		}
		if (prev && sameApartFromOutput(prev, next)) {
			// Output-only: stamp in place, silently, as the old machine did.
			prev.lastOutputAt = next.lastOutputAt;
		} else {
			records.set(id, next);
			changed = true;
		}
	}

	const prevSubs = subagentLists.get(id) ?? EMPTY_SUBAGENTS;
	const subsChanged = !sameJson(prevSubs, u.subagents);
	if (subsChanged) {
		if (u.subagents.length === 0 || u.view === null) subagentLists.delete(id);
		else subagentLists.set(id, u.subagents);
	}
	const prevTasks = taskLists.get(id) ?? EMPTY_TASKS;
	const tasksChanged = !sameJson(prevTasks, u.backgroundTasks);
	if (tasksChanged) {
		if (u.backgroundTasks.length === 0 || u.view === null) taskLists.delete(id);
		else taskLists.set(id, u.backgroundTasks);
	}

	if (changed || subsChanged || tasksChanged) fire(harnessListeners, id);
	if (subsChanged) fire(subagentListeners, id);
	if (tasksChanged) fire(taskListeners, id);
	recomputePermissionIds();

	dispatchEvents(u);
	return true;
}

export function applySnapshot(us: readonly PhaseUpdate[]): void {
	for (const u of us) applyUpdate(u);
}

/// Stamp the local PTY output time on a mirrored record. Silent. An exited
/// record is left alone.
export function stampOutput(id: string, at: number): void {
	const rec = records.get(id);
	if (rec && rec.phase !== "exited") rec.lastOutputAt = at;
}

export const phaseMirror = {
	get: (id: string): HarnessActivity | null => records.get(id) ?? null,
	ids: (): string[] => [...records.keys()],
	subagents: (id: string): readonly SubagentEntry[] => subagentLists.get(id) ?? EMPTY_SUBAGENTS,
	backgroundTasks: (id: string): readonly BackgroundTaskEntry[] => taskLists.get(id) ?? EMPTY_TASKS,
	permissionIds: (): ReadonlySet<string> => permissionIds,
	subscribe: (id: string, cb: () => void): (() => void) => addTo(harnessListeners, id, cb),
	subscribeSubagents: (id: string, cb: () => void): (() => void) =>
		addTo(subagentListeners, id, cb),
	subscribeTasks: (id: string, cb: () => void): (() => void) => addTo(taskListeners, id, cb),
	subscribePermissionIds(cb: () => void): () => void {
		permissionListeners.add(cb);
		return () => {
			permissionListeners.delete(cb);
		};
	},
	subscribeTransitions(cb: TransitionCb): () => void {
		transitionListeners.add(cb);
		return () => {
			transitionListeners.delete(cb);
		};
	},
	subscribeDelegationDeferred(cb: (id: string) => void): () => void {
		deferralListeners.add(cb);
		return () => {
			deferralListeners.delete(cb);
		};
	},
	/// Overdue tasks (attach-only excluded) no earlier ceiling notice has
	/// announced; marks them announced. Silent.
	takeUnreportedOverdue(id: string): number {
		let seen = announcedOverdue.get(id);
		let n = 0;
		for (const t of taskLists.get(id) ?? EMPTY_TASKS) {
			if (t.fromAttach || !t.overdue || seen?.has(t.taskId)) continue;
			if (!seen) {
				seen = new Set();
				announcedOverdue.set(id, seen);
			}
			seen.add(t.taskId);
			n++;
		}
		return n;
	},
};

/// Test-only: forget everything.
export function resetPhaseMirror(): void {
	records.clear();
	subagentLists.clear();
	taskLists.clear();
	lastSeq.clear();
	announcedOverdue.clear();
	permissionIds = new Set();
}

export interface MirrorDeps {
	listen: (cb: (u: PhaseUpdate) => void) => Promise<UnlistenFn>;
	snapshot: () => Promise<PhaseUpdate[]>;
	delay: (ms: number) => Promise<void>;
	log: (message: string) => void;
	/// Clock for the log throttle; defaults to `Date.now`.
	now?: () => number;
}

const defaultDeps: MirrorDeps = {
	listen: (cb) => listen<PhaseUpdate>(HARNESS_PHASE_EVENT, (e) => cb(e.payload)),
	snapshot: () => invoke<PhaseUpdate[]>("harness_phase_snapshot"),
	delay: (ms) => new Promise((r) => setTimeout(r, ms)),
	log: (m) => logToRust("warn", "skein::phase_mirror", m),
};

/// The first this many attempts are `SNAPSHOT_RETRY_MS` apart, then
/// `SNAPSHOT_SLOW_RETRY_MS` for as long as the app runs: the daemon publishes
/// only on change, so a harness sitting in `waiting` appears only through the
/// snapshot.
export const SNAPSHOT_ATTEMPTS = 5;
export const SNAPSHOT_RETRY_MS = 1_000;
export const SNAPSHOT_SLOW_RETRY_MS = 5_000;
export const FAILURE_LOG_INTERVAL_MS = 60_000;

let started: Promise<void> | null = null;
let firstSnapshotApplied = false;
const firstSnapshotListeners = new Set<() => void>();

/// One-shot: `cb` runs once, when the first snapshot has been applied (the
/// records of harnesses that were already settled when the app loaded exist
/// from then on, with no transition to announce them). If that already
/// happened, `cb` is not called; the caller reads the mirror directly. Returns
/// an unsubscribe.
export function onFirstSnapshot(cb: () => void): () => void {
	if (firstSnapshotApplied) return () => {};
	firstSnapshotListeners.add(cb);
	return () => {
		firstSnapshotListeners.delete(cb);
	};
}

function markSnapshotApplied(): void {
	if (firstSnapshotApplied) return;
	firstSnapshotApplied = true;
	const cbs = [...firstSnapshotListeners];
	firstSnapshotListeners.clear();
	for (const cb of cbs) safely(cb);
}

/// Start once at app boot: listen and buffer, fetch the snapshot, apply it,
/// then the buffered events newer than it, then apply live events directly.
/// A failed listen or snapshot is retried for as long as the app runs (events
/// apply live meanwhile). Resolves when the snapshot is in.
export function startPhaseMirror(deps: MirrorDeps = defaultDeps): Promise<void> {
	if (started) return started;
	started = run(deps);
	return started;
}

/// Test-only: allow `startPhaseMirror` to run again.
export function resetPhaseMirrorStart(): void {
	started = null;
	firstSnapshotApplied = false;
	firstSnapshotListeners.clear();
}

async function run(deps: MirrorDeps): Promise<void> {
	const now = deps.now ?? Date.now;
	const lastLogged = new Map<string, number>();
	const logFailure = (kind: string, message: string): void => {
		const at = now();
		const prev = lastLogged.get(kind);
		if (prev !== undefined && at - prev < FAILURE_LOG_INTERVAL_MS) return;
		lastLogged.set(kind, at);
		deps.log(message);
	};
	let buffer: PhaseUpdate[] | null = [];
	const onUpdate = (u: PhaseUpdate): void => {
		if (buffer) buffer.push(u);
		else applyUpdate(u);
	};
	const flushHeld = (): void => {
		const held = buffer ?? [];
		buffer = null;
		for (const u of held) applyUpdate(u);
	};
	const pause = (attempt: number): Promise<void> =>
		deps.delay(attempt < SNAPSHOT_ATTEMPTS ? SNAPSHOT_RETRY_MS : SNAPSHOT_SLOW_RETRY_MS);

	// Listen first. While it keeps failing, still try the snapshot on the same
	// schedule: it carries the records the send gate needs.
	for (let attempt = 1; ; attempt++) {
		try {
			await deps.listen(onUpdate);
			break;
		} catch (e) {
			logFailure("listen", `phase listen failed (attempt ${attempt}): ${String(e)}`);
			try {
				applySnapshot(await deps.snapshot());
				markSnapshotApplied();
			} catch (err) {
				logFailure(
					"snapshot",
					`harness_phase_snapshot failed (attempt ${attempt}): ${String(err)}`,
				);
			}
			await pause(attempt);
		}
	}
	// Listening now: this snapshot covers everything before the listener.
	for (let attempt = 1; ; attempt++) {
		try {
			const snap = await deps.snapshot();
			const snapSeq = snap.reduce((m, u) => Math.max(m, u.seq), -1);
			applySnapshot(snap);
			const held = buffer ?? [];
			buffer = null;
			// An update the snapshot already covers keeps its transitions (the
			// snapshot carries none) but must not touch state: its harness may
			// have been forgotten since, and applying it would bring it back.
			for (const u of held) {
				if (u.seq > snapSeq) applyUpdate(u);
				else dispatchEvents(u);
			}
			markSnapshotApplied();
			return;
		} catch (e) {
			logFailure("snapshot", `harness_phase_snapshot failed (attempt ${attempt}): ${String(e)}`);
			// Live events apply meanwhile; whatever was buffered goes in now.
			flushHeld();
			await pause(attempt);
		}
	}
}
