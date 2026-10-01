// Per-harness activity state machine + event hook (epic #50, L1+L3+L2a).
//
// The single source of truth for "what is this harness doing right
// now?" — read by the bottom status bar (#29), by the harness tab
// dots, and (in follow-on PRs) by the notification surfaces (#12),
// the per-room aggregate (#50 L4), and the cross-harness activity
// feed (#50 L7).
//
// Today we have exactly one signal flowing from a harness: bytes
// over the PTY `Channel<String>`. This module derives a real state
// machine from that signal using the idle-heuristic strategy (#50
// L2a): output → `running`, sustained silence → `idle`, PTY exit
// → `exited`. Pattern-match and harness-native strategies (L2b /
// L2c) will plug in later by calling the same `setPhase` mutator —
// consumers won't know or care which strategy fed the transition.
//
// Why not Rust-side: this state is purely derived from a stream
// the frontend already receives. Putting the model in Rust would
// add a second IPC channel and split logic across the boundary
// for no win. If we ever need cross-restart persistence (epic L6)
// the natural shape is "frontend emits transitions, Rust appends
// to a log" — the state machine itself can stay here.
//
// #19: this module used to be one ~1450-line file. It is now the
// entry point — the `harnessActivity` store object plus re-exports —
// with the pieces that don't need to share its closure split out:
// harnessActivityTypes.ts (ActivityPhase/HarnessActivity/TransitionSource
// /TRANSITION_SOURCE), harnessActivityConstants.ts (the tuning
// thresholds), harnessActivityCore.ts (the mutable store state, setPhase,
// the background tick, the #259/#273 watchdog degrade helpers, #277's
// disarmDelegation, isDecisiveInput), harnessActivityLabels.ts (pure
// status/label derivation) and useHarnessActivity.ts (the React hooks).
// #458 then split the store object's methods by the question a reader
// has: harnessActivityLifecycle.ts (spawn/exit/forget, adapter
// authority), harnessActivityDeferral.ts (adapter work/end-of-turn
// signals and the #277/#446 deferral), harnessActivityPermission.ts
// (#86), harnessActivityIo.ts (user input, PTY output) and
// harnessActivitySupervisor.ts (#404 silence recovery, #423 repairs).
// Reads and subscriptions stay here.
// Every symbol this module exported before the split is still exported
// from here, so `import { ... } from "./harnessActivity.ts"` is unchanged
// for every caller.

import {
	delegationDeferredListeners,
	listeners,
	phaseSnapshot,
	store,
	transitionListeners,
} from "./harnessActivityCore.ts";
import { deferralMethods } from "./harnessActivityDeferral.ts";
import { ioMethods } from "./harnessActivityIo.ts";
import { lifecycleMethods } from "./harnessActivityLifecycle.ts";
import { permissionMethods } from "./harnessActivityPermission.ts";
import { supervisorMethods } from "./harnessActivitySupervisor.ts";
import type { ActivityPhase, HarnessActivity, TransitionListener } from "./harnessActivityTypes.ts";

export type {
	ActivityPhase,
	HarnessActivity,
	TransitionListener,
	TransitionSource,
} from "./harnessActivityTypes.ts";
export { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
export {
	atSafeStoppingPoint,
	isDecisiveInput,
	onTick,
	phaseSnapshot,
} from "./harnessActivityCore.ts";
export {
	activityToStatus,
	aggregateRoomStatus,
	delegationSummary,
	effectiveStatus,
	higherPriorityStatus,
	statusLabel,
	stillRunningSummary,
} from "./harnessActivityLabels.ts";
export type { RoomHarnessRef } from "./harnessActivityLabels.ts";
export {
	useHarnessActivity,
	usePermissionHarnessIds,
	useRoomActivity,
} from "./useHarnessActivity.ts";

export const harnessActivity = {
	...lifecycleMethods,
	...deferralMethods,
	...permissionMethods,
	...ioMethods,
	...supervisorMethods,

	get(id: string): HarnessActivity | null {
		return store.get(id) ?? null;
	},

	/// #423: every harness id in the store, for the supervisor's sweep.
	ids(): string[] {
		return [...store.keys()];
	},

	/// #356: every harness id this process has a phase for, for the
	/// `harness_phases` agent-request kind. See `phaseSnapshot`'s own
	/// doc for why an id can be missing from the result.
	phaseSnapshot(): Record<string, ActivityPhase> {
		return phaseSnapshot(store);
	},

	subscribe(id: string, cb: () => void): () => void {
		let set = listeners.get(id);
		if (!set) {
			set = new Set();
			listeners.set(id, set);
		}
		set.add(cb);
		return () => {
			const s = listeners.get(id);
			if (!s) return;
			s.delete(cb);
			if (s.size === 0) listeners.delete(id);
		};
	},

	/// Subscribe to every phase transition across every harness.
	/// One callback fires for each real transition with `(id, from,
	/// to)`. Returns an unsubscribe. Used by App-level notification
	/// logic that fans out to multiple rooms.
	subscribeTransitions(cb: TransitionListener): () => void {
		transitionListeners.add(cb);
		return () => {
			transitionListeners.delete(cb);
		};
	},

	/// #381: subscribe to #277 delegation deferrals ARMING — `cb(id)`
	/// fires the instant a harness's end-of-turn is deferred for
	/// background subagents (`awaitingPromptFromAdapter`'s
	/// `delegationDeferredAt` going null → non-null), never on the
	/// idempotent re-arm and never on disarm. Step B (mail delivery
	/// during a deferral) is the intended caller — see
	/// `atSafeStoppingPoint` for the companion "is it still safe to send
	/// into right now" check. Returns an unsubscribe, same shape as
	/// `subscribeTransitions`.
	subscribeDelegationDeferred(cb: (id: string) => void): () => void {
		delegationDeferredListeners.add(cb);
		return () => {
			delegationDeferredListeners.delete(cb);
		};
	},
};
