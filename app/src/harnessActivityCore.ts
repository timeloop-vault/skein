// The harness activity store's mutable module state + engine (epic #50,
// L1+L3+L2a; #259 watchdog; #273 launch-silent watchdog; #277 delegation
// deferral). Split out of harnessActivity.ts (#19) — this is the single
// instance every other module (the `harnessActivity` object, the React
// hooks) reads and writes through the exports below. See harnessActivity.ts
// for the module this belongs to.

import {
	ADAPTER_SILENT_AFTER_MS,
	DELEGATION_CEILING_MS,
	DELEGATION_SETTLE_MS,
	IDLE_AFTER_MS,
	LAUNCH_SILENT_AFTER_MS,
	PATTERN_WAITING_AFTER_MS,
	TICK_INTERVAL_MS,
} from "./harnessActivityConstants.ts";
import { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
import type {
	ActivityPhase,
	HarnessActivity,
	TransitionListener,
	TransitionSource,
} from "./harnessActivityTypes.ts";
import { matchesWaitingPrompt } from "./harnessPatterns.ts";
import { subagents } from "./subagents.ts";

export const store = new Map<string, HarnessActivity>();
export const listeners = new Map<string, Set<() => void>>();
export const muteUntil = new Map<string, number>();
let tickHandle: ReturnType<typeof setInterval> | null = null;

export const transitionListeners = new Set<TransitionListener>();

export const emit = (id: string): void => {
	const set = listeners.get(id);
	if (!set) return;
	for (const cb of set) cb();
};

/// Set equality for the permission-ids snapshot below — only used to
/// decide whether a fresh Set needs allocating, since
/// `useSyncExternalStore` requires a referentially stable snapshot
/// when nothing actually changed.
const setsEqual = (a: ReadonlySet<string>, b: ReadonlySet<string>): boolean => {
	if (a.size !== b.size) return false;
	for (const id of a) if (!b.has(id)) return false;
	return true;
};

/// Every harness id currently in `permission`, across every room.
/// Rebuilt (not incrementally maintained) on every transition that
/// touches the phase — the store is small enough that a full scan is
/// cheap, and it keeps this correct by construction instead of by
/// careful bookkeeping at each of the several call sites that can
/// enter or leave `permission`. #86.
export let permissionIds: ReadonlySet<string> = new Set();
export const permissionListeners = new Set<() => void>();

export const recomputePermissionIds = (): void => {
	const next = new Set<string>();
	for (const [id, a] of store) {
		if (a.phase === "permission") next.add(id);
	}
	if (setsEqual(next, permissionIds)) return;
	permissionIds = next;
	for (const cb of permissionListeners) cb();
};

export const setPhase = (
	id: string,
	phase: ActivityPhase,
	source: TransitionSource,
	patch?: Partial<HarnessActivity>,
): void => {
	const cur = store.get(id);
	if (!cur) return;
	// Suppress no-op transitions so consumers don't churn on
	// continuous output (every chunk would otherwise emit). Only
	// real phase changes notify subscribers; `lastOutputAt`
	// mutates silently inside `recordOutput`. A patch-only call (e.g.
	// re-entering `permission` with a fresh tool name) still goes
	// through even when the phase itself didn't change, so `.get()`
	// reflects the new tool.
	if (cur.phase === phase && !patch) return;
	const from = cur.phase;
	// Leaving `permission` drops the stale tool name by default; a
	// caller reporting a fresh one (re-entering `permission`) passes
	// it in `patch`, which is spread after and wins.
	const leftPermission = from === "permission" && phase !== "permission";
	store.set(id, {
		...cur,
		phase,
		...(leftPermission
			? { permissionTool: null, permissionAgentType: null, permissionAgentId: null }
			: null),
		...patch,
	});
	emit(id);
	if (from !== phase) {
		for (const cb of transitionListeners) cb(id, from, phase, source);
	}
	if (from === "permission" || phase === "permission") {
		recomputePermissionIds();
	}
};

/// The adapter never spoke after a prompt was submitted: give up on it.
/// Without this an adapter watching the wrong place freezes the harness
/// for good — authority silences PTY output, the tick skips it, and it
/// never leaves `spawning` (#259, a JSONL path encoded differently from
/// Claude's). `spawning` is moved on explicitly because the tick below
/// only ever acts on `running`; `lastOutputAt` restarts so the idle
/// window is measured from here, not from spawn.
const degradeSilentAdapter = (id: string, cur: HarnessActivity, now: number): void => {
	console.warn(
		`[skein] harness ${id}: adapter delivered nothing ${ADAPTER_SILENT_AFTER_MS / 1000}s after a prompt; falling back to the idle heuristic`,
	);
	store.set(id, {
		...cur,
		authoritative: false,
		adapterSilent: true,
		degradedBy: "adapter-silent",
		lastOutputAt: now,
	});
	if (cur.phase === "spawning") {
		setPhase(id, "running", TRANSITION_SOURCE.AdapterSilent);
	}
};

/// #273: neither the tail nor the launch hook has said anything at all,
/// this long after spawn, and the harness is still `spawning`: give up
/// on the adapter the same way `degradeSilentAdapter` does, but under
/// its own transition source (`LaunchSilent`) because the two are
/// diagnosed differently — this one needed no prompt to arm, it fires
/// on pure silence since spawn. Sets `adapterSilent` (not a separate
/// flag) so `noteLaunchSignal`'s recovery path — the same one
/// `adapterDelivered` already uses — applies here too if the launch
/// signal turns up late (e.g. the user finally accepts a trust
/// dialog). Unlike `degradeSilentAdapter`, the phase write here is
/// unconditional: the caller's guard already confirmed `phase ===
/// "spawning"`.
const degradeLaunchSilentAdapter = (id: string, cur: HarnessActivity, now: number): void => {
	console.warn(
		`[skein] harness ${id}: no launch signal ${LAUNCH_SILENT_AFTER_MS / 1000}s after spawn; falling back to the idle heuristic`,
	);
	store.set(id, {
		...cur,
		authoritative: false,
		adapterSilent: true,
		degradedBy: "launch-silent",
		lastOutputAt: now,
	});
	setPhase(id, "running", TRANSITION_SOURCE.LaunchSilent);
};

/// #277: void an armed end-of-turn deferral. Called from every
/// main-transcript work/settle signal — `setRunningFromAdapter`,
/// `setWaitingFromAdapter`, `exited` — so the deferral can never
/// survive past whatever event proves the session isn't waiting on
/// its subagents anymore. Mutates in place, the way `recordOutput`
/// bumps `lastOutputAt`: this is bookkeeping, not a phase change, so
/// it must not emit — a caller that goes on to call `setPhase` gets
/// its own emit from that.
///
/// Deliberately unconditional on the harness's current phase: Rule 2
/// requires this to run even when the caller it's called from is
/// about to no-op (`setRunningFromAdapter` while still `permission`
/// without `clearsPermission`, or `setPhase`'s own same-phase
/// short-circuit) — the disarm must not depend on a phase write
/// actually happening.
export const disarmDelegation = (id: string): void => {
	const cur = store.get(id);
	if (!cur || cur.delegationDeferredAt === null) return;
	cur.delegationDeferredAt = null;
	cur.delegationEmptiedAt = null;
};

export const ensureTick = (): void => {
	if (tickHandle !== null) return;
	tickHandle = setInterval(() => {
		const now = Date.now();
		for (const [id, a] of store) {
			// #277: a deferred end-of-turn is evaluated before the
			// `authoritative` skip below, because a deferred harness
			// IS authoritative by definition — only the Claude
			// translator's `awaitingPromptFromAdapter` arms one, and
			// only while an L2c adapter is attached. `continue`s out
			// either way, so the authoritative block never
			// double-handles it.
			if (a.delegationDeferredAt !== null) {
				// #86: permission is the harder stop. Leave the
				// harness alone; re-evaluate on a later tick once the
				// dialog clears back to `running`.
				if (a.phase === "permission") continue;
				const working = subagents.workingCount(id);
				if (working === 0) {
					// Rule 3 — settle timer. See `DELEGATION_SETTLE_MS`
					// for the measurement behind the threshold.
					if (a.delegationEmptiedAt === null) {
						a.delegationEmptiedAt = now;
					} else if (now - a.delegationEmptiedAt >= DELEGATION_SETTLE_MS) {
						a.delegationDeferredAt = null;
						a.delegationEmptiedAt = null;
						setPhase(id, "waiting", TRANSITION_SOURCE.DelegationSettled);
					}
				} else {
					// The working set is non-empty again after having
					// been seen empty — clear the mark so a fresh
					// settle window starts cleanly if it empties again.
					if (a.delegationEmptiedAt !== null) a.delegationEmptiedAt = null;
					// Rule 4 — ceiling. See `DELEGATION_CEILING_MS` for
					// the measurement behind the threshold.
					if (now - a.delegationActivityAt >= DELEGATION_CEILING_MS) {
						const dropped = subagents.presumeGone(id);
						console.warn(
							`[skein] harness ${id}: presumed ${dropped} subagent(s) gone after ` +
								`${DELEGATION_CEILING_MS / 60_000} min with no subagent event; flushing the deferred end of turn`,
						);
						a.delegationDeferredAt = null;
						a.delegationEmptiedAt = null;
						setPhase(id, "waiting", TRANSITION_SOURCE.DelegationCeiling);
					}
				}
				continue;
			}
			// Harnesses with an authoritative source (Claude JSONL
			// tail, opencode SSE adapter) write their own phase
			// from the L2c module. The idle heuristic would fight
			// the adapter — e.g. when Claude is mid-turn but the
			// PTY is briefly quiet during a tool call, L2a would
			// flip to `idle` while L2c says `running`. Skip these,
			// unless the adapter has proven silent (#259).
			if (a.authoritative) {
				if (
					!a.adapterHeard &&
					a.promptSubmittedAt !== null &&
					now - a.promptSubmittedAt >= ADAPTER_SILENT_AFTER_MS
				) {
					degradeSilentAdapter(id, a, now);
				} else if (
					// #273: mirrors the #259 check's own `!a.adapterHeard`
					// guard — if the tail has already proven itself, this
					// timer has nothing to add, so an `else if` keeps the
					// two mutually exclusive rather than both firing (and
					// double-transitioning) in the same tick.
					!a.adapterHeard &&
					a.launchSignalAt === null &&
					a.phase === "spawning" &&
					now - a.spawnedAt >= LAUNCH_SILENT_AFTER_MS
				) {
					degradeLaunchSilentAdapter(id, a, now);
				}
				continue;
			}
			if (a.phase !== "running" || a.lastOutputAt === null) continue;
			const quiet = now - a.lastOutputAt;
			// L2b: pattern-match → waiting on a shorter threshold
			// than idle. Triggered when output has been quiet for
			// PATTERN_WAITING_AFTER_MS (500 ms) AND the tail buffer
			// matches a known prompt regex (sudo password, [y/n],
			// "Press Enter", …). The shorter threshold makes the
			// dot flip blue near-instantly when a sudo prompt
			// appears, instead of waiting the full 8 s idle window.
			//
			// If the tail doesn't match a known prompt, the
			// harness falls through to the L2a `→ idle` check
			// below — same behaviour as pre-L2b.
			if (quiet >= PATTERN_WAITING_AFTER_MS && matchesWaitingPrompt(a.tail)) {
				setPhase(id, "waiting", TRANSITION_SOURCE.L2bPattern);
				continue;
			}
			if (quiet >= IDLE_AFTER_MS) {
				setPhase(id, "idle", TRANSITION_SOURCE.L2aIdle);
			}
		}
	}, TICK_INTERVAL_MS);
};

export const stopTickIfIdle = (): void => {
	if (store.size === 0 && tickHandle !== null) {
		clearInterval(tickHandle);
		tickHandle = null;
	}
};

/// Is this keystroke "decisive" enough to count as answering a
/// permission dialog (#86)? Claude gives us no explicit "answered"
/// signal — approval and denial both just let the tool proceed or
/// error out, and that can take minutes — so the only prompt signal
/// left is the user's own keystroke. Deliberately narrow: a lone ESC
/// (dismiss) counts, but an escape *sequence* (arrow keys `\x1b[A`,
/// xterm's focus events `\x1b[I` / `\x1b[O`) must not, since those
/// fire on ordinary navigation and window-focus changes that have
/// nothing to do with answering anything. Exported and pure so it can
/// be unit tested without the store.
export function isDecisiveInput(data: string): boolean {
	if (data.includes("\r") || data.includes("\n")) return true;
	if (data === "\x1b") return true;
	if (data.includes("\x03")) return true;
	if (data.length === 1 && /^[0-9yYnN]$/.test(data)) return true;
	return false;
}
