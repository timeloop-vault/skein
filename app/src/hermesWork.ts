// #628: outstanding delegated work for a Hermes harness. Two sources feed
// the same registries the Claude adapter does (`subagents.ts`,
// `backgroundTasks.ts`), so the existing end-of-turn deferral (#277/#446)
// holds a Hermes turn at "delegating" instead of "waiting":
// - the `subagent_start` / `subagent_stop` hooks (live, via
//   `hermesHookAdapter.ts`), and
// - the Rust disk stream `hermes_work_attach` (`HermesWorkEvent`): an attach
//   snapshot (`initial: true`) then live diffs, covering background
//   `delegate_task` children AND background terminal processes, which have
//   no hook at all.
// Both sources may report the same delegation; `record` and `finish` are
// idempotent (finish of an unknown id is a no-op), and the live-start notes
// below run only the first time an id is seen live, so double-reporting is
// harmless. Pure mapping, no Tauri: the invoke wrapper takes the channel
// from its caller.

import { backgroundTasks } from "./backgroundTasks.ts";
import { harnessActivity } from "./harnessActivity.ts";
import { subagents } from "./subagents.ts";

/// Mirror of the Rust enum (serde tag `kind`, camelCase). Keep in lock-step.
export type HermesWorkEvent =
	| { kind: "subagentStart"; agentId: string; description: string | null; initial: boolean }
	| { kind: "subagentEnd"; agentId: string }
	| { kind: "backgroundStart"; taskId: string; command: string | null; initial: boolean }
	| { kind: "backgroundEnd"; taskId: string }
	/// No running delegation with a live owner is left (an edge, never the
	/// first pass): finishes every subagent, whatever reported it, to cover
	/// a lost `subagent_stop` hook.
	| { kind: "delegationsIdle" };

/// A subagent began (or was found running). Only a LIVE start the registry
/// has not already counted as live feeds `delegatedCount` and proof-of-life,
/// like the Claude `subagent_start` arm.
export function startSubagent(
	harnessId: string,
	agentId: string,
	description: string | null,
	initial: boolean,
): void {
	if (!initial && recentlyEnded(harnessId, agentId)) return;
	const known = subagents.live(harnessId).find((s) => s.agentId === agentId);
	const alreadyLive = known !== undefined && !known.fromAttach;
	subagents.record(harnessId, { agentId, agentType: null, description }, initial);
	if (initial || alreadyLive) return;
	harnessActivity.noteSubagentStarted(harnessId);
	harnessActivity.reclaimDelegationFromAdapter(harnessId);
}

/// The registry has no tombstone: a hook's start+stop followed by the disk
/// poller's first sighting of the same child (its row still unended for a
/// moment) would re-add a finished child. A live start this soon after its
/// end is that echo, so it is ignored.
export const RECENTLY_ENDED_MS = 3_000;
const endedAt = new Map<string, Map<string, number>>();

function recentlyEnded(harnessId: string, agentId: string): boolean {
	const m = endedAt.get(harnessId);
	if (!m) return false;
	const now = Date.now();
	for (const [id, t] of m) if (now - t >= RECENTLY_ENDED_MS) m.delete(id);
	return m.has(agentId);
}

export function endSubagent(harnessId: string, agentId: string): void {
	const m = endedAt.get(harnessId) ?? new Map<string, number>();
	endedAt.set(harnessId, m);
	m.set(agentId, Date.now());
	subagents.finish(harnessId, agentId);
	harnessActivity.noteSubagentActivity(harnessId);
}

/// Map one stream event onto the registries.
export function applyHermesWork(harnessId: string, e: HermesWorkEvent): void {
	switch (e.kind) {
		case "subagentStart":
			startSubagent(harnessId, e.agentId, e.description, e.initial);
			return;
		case "subagentEnd":
			endSubagent(harnessId, e.agentId);
			return;
		case "backgroundStart":
			// "bash": no deadline (not a Monitor), so it gets the plain
			// BACKGROUND_TASK_CEILING_MS rule. Hermes reports no timeout.
			backgroundTasks.record(
				harnessId,
				{
					taskId: e.taskId,
					kind: "bash",
					description: null,
					command: e.command,
					timeoutMs: null,
					persistent: false,
					agentId: null,
				},
				e.initial,
			);
			if (!e.initial) harnessActivity.reclaimDelegationFromAdapter(harnessId);
			return;
		case "backgroundEnd":
			backgroundTasks.finish(harnessId, e.taskId);
			return;
		case "delegationsIdle":
			for (const s of subagents.live(harnessId)) endSubagent(harnessId, s.agentId);
			return;
	}
}

/// Drop every delegated-work entry of this harness (session change or
/// teardown). The deferral needs no nudge: the next tick sees the empty
/// working set and runs its ordinary settle window.
export function clearHermesWork(harnessId: string): void {
	endedAt.delete(harnessId);
	subagents.forget(harnessId);
	backgroundTasks.forget(harnessId);
}
