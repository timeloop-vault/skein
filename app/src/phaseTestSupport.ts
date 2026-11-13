// Test helpers for code that reads the phase mirror (#732): build a
// `PhaseUpdate` and apply it, the way the daemon's stream would.

import type { BackgroundTaskEntry } from "./backgroundTasks.ts";
import type { ActivityPhase, HarnessActivity, TransitionSource } from "./harnessActivityTypes.ts";
import { applyUpdate, type PhaseTransition, type PhaseUpdate } from "./phaseMirror.ts";
import type { SubagentEntry } from "./subagents.ts";

export type WireActivity = HarnessActivity;

let seq = 0;

export function wireActivity(phase: ActivityPhase, over: Partial<WireActivity> = {}): WireActivity {
	return {
		phase,
		phaseSince: 0,
		spawnedAt: 0,
		hasUserInput: false,
		permissionTool: null,
		permissionAgentType: null,
		delegatedCount: 0,
		lastTurnSignal: null,
		lastSubmitAt: null,
		authoritative: false,
		adapterHeard: false,
		adapterSilent: false,
		launchSignalAt: null,
		lastOutputAt: null,
		questionOpen: false,
		delegationDeferredAt: null,
		injected: false,
		...over,
	};
}

/// A full `HarnessActivity`, for pure-function tests.
export function mkActivity(
	phase: ActivityPhase,
	over: Partial<HarnessActivity> = {},
): HarnessActivity {
	return wireActivity(phase, over);
}

export function subagentEntry(agentId: string, over: Partial<SubagentEntry> = {}): SubagentEntry {
	return { agentId, agentType: null, description: null, startedAt: 1, fromAttach: false, ...over };
}

export function taskEntry(
	taskId: string,
	over: Partial<BackgroundTaskEntry> = {},
): BackgroundTaskEntry {
	return {
		taskId,
		kind: "bash",
		description: null,
		command: null,
		timeoutMs: null,
		persistent: false,
		agentId: null,
		startedAt: 1,
		fromAttach: false,
		overdue: false,
		overdueReported: false,
		...over,
	};
}

export function transitionOf(
	harnessId: string,
	from: ActivityPhase,
	to: ActivityPhase,
	source: TransitionSource = "test",
): PhaseTransition {
	return { harnessId, from, to, source, atMs: 0, hasUserInput: false };
}

/// An update with the next global seq. `view: null` forgets the harness.
export function phaseUpdate(id: string, over: Partial<PhaseUpdate> = {}): PhaseUpdate {
	return {
		seq: ++seq,
		harnessId: id,
		roomId: "room",
		view: wireActivity("waiting"),
		subagents: [],
		backgroundTasks: [],
		transitions: [],
		delegationArmed: false,
		...over,
	};
}

/// Put a harness in `phase` in the mirror, replacing its record and lists.
export function mirrorSet(
	id: string,
	phase: ActivityPhase,
	over: Partial<WireActivity> = {},
	extra: Partial<PhaseUpdate> = {},
): void {
	applyUpdate(phaseUpdate(id, { view: wireActivity(phase, over), ...extra }));
}

/// Remove a harness from the mirror, as a daemon "forgotten" update does.
export function mirrorForget(id: string): void {
	applyUpdate(phaseUpdate(id, { view: null }));
}

/// The one state `canSendPrompt` allows once the phase is right: authoritative,
/// heard-from and injected.
export const SENDABLE: Partial<WireActivity> = {
	authoritative: true,
	adapterHeard: true,
	injected: true,
};
