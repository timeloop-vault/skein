// The webview's view of "what is this harness doing right now?" (epic #50).
//
// Since #732 the daemon's `PhaseService` is the authority; this is a mirror
// of its `skein://harness-phase` stream (`phaseMirror.ts`). Reads and
// subscriptions are served from the mirror. The client-only inputs the
// daemon cannot see (keystrokes, a submitted prompt, a launch hook) are
// forwarded to it; it hears adapter events, PTY output, spawn and exit
// itself. Pure derivation (labels, status) lives in
// harnessActivityLabels.ts, the hooks in useHarnessActivity.ts.

import type { ActivityPhase, HarnessActivity, TransitionListener } from "./harnessActivityTypes.ts";
import { sendPhaseInput } from "./phaseInput.ts";
import { phaseMirror, stampOutput } from "./phaseMirror.ts";

export type { RoomHarnessRef } from "./harnessActivityLabels.ts";
export {
	activityToStatus,
	aggregateRoomStatus,
	atSafeStoppingPoint,
	delegationSummary,
	effectiveStatus,
	harnessDisplayStatus,
	higherPriorityStatus,
	statusLabel,
	stillRunningSummary,
} from "./harnessActivityLabels.ts";
export type {
	ActivityPhase,
	HarnessActivity,
	TransitionListener,
	TransitionSource,
} from "./harnessActivityTypes.ts";
export { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
export {
	useHarnessActivity,
	usePermissionHarnessIds,
	useRoomActivity,
} from "./useHarnessActivity.ts";

export const harnessActivity = {
	// Client-only inputs: forwarded to the daemon, no local state.
	recordInput(id: string, data: string, opts?: { emptyComposer?: boolean }): void {
		sendPhaseInput(id, { kind: "input", data, emptyComposer: opts?.emptyComposer ?? false });
	},

	notePromptSubmitted(id: string): void {
		sendPhaseInput(id, { kind: "promptSubmitted" });
	},

	muteInducedOutput(id: string): void {
		sendPhaseInput(id, { kind: "muteInducedOutput" });
	},

	/// Claude's SessionStart hook reached the webview.
	noteLaunchSignal(id: string): void {
		sendPhaseInput(id, { kind: "launchSignal" });
	},

	sessionSwitched(id: string, source: "clear" | "resume" | "fork" | "startup"): void {
		sendPhaseInput(id, { kind: "sessionSwitched", switch: source });
	},

	recoverUnheardSilence(id: string): void {
		sendPhaseInput(id, { kind: "recoverUnheardSilence" });
	},

	/// PTY output arrived. The daemon throttles `lastOutputAt`, so the local
	/// stamp (silent, as it always was) keeps it fresh between updates.
	recordOutput(id: string, _chunk?: string): void {
		stampOutput(id, Date.now());
	},

	get(id: string): HarnessActivity | null {
		return phaseMirror.get(id);
	},

	/// Every harness id mapped to its phase; an id the daemon does not know
	/// is absent.
	phaseSnapshot(): Record<string, ActivityPhase> {
		const phases: Record<string, ActivityPhase> = {};
		for (const id of phaseMirror.ids()) {
			const a = phaseMirror.get(id);
			if (a) phases[id] = a.phase;
		}
		return phases;
	},

	subscribe(id: string, cb: () => void): () => void {
		return phaseMirror.subscribe(id, cb);
	},

	/// Subscribe to every phase transition across every harness:
	/// `(id, from, to, source)`. Returns an unsubscribe.
	subscribeTransitions(cb: TransitionListener): () => void {
		return phaseMirror.subscribeTransitions(cb);
	},

	/// #381: `cb(id)` fires when a harness's end-of-turn is deferred for
	/// background work (a #277 deferral arming), never on disarm.
	subscribeDelegationDeferred(cb: (id: string) => void): () => void {
		return phaseMirror.subscribeDelegationDeferred(cb);
	},
};
