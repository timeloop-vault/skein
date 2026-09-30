// The end-of-turn deferral's tick rules (#277, generalised by #446 + #441).
// Split out of harnessActivityCore.ts. Deliberately imports nothing from
// the core or the public entry (an import cycle): the tick passes in the
// activity record, the clock and its own `setPhase`.
//
// "Outstanding work" is subagents PLUS background tasks. Either one
// holding a main session's end of turn keeps the harness `running`
// instead of a premature `waiting`.

import { backgroundTasks, deadlineOf } from "./backgroundTasks.ts";
import {
	BACKGROUND_TASK_CEILING_MS,
	DELEGATION_CEILING_MS,
	DELEGATION_SETTLE_MS,
} from "./harnessActivityConstants.ts";
import { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
import type { ActivityPhase, HarnessActivity, TransitionSource } from "./harnessActivityTypes.ts";
import { subagents } from "./subagents.ts";

/// Working subagents plus working background tasks for one harness.
export function outstandingWork(id: string): number {
	return subagents.workingCount(id) + backgroundTasks.workingCount(id);
}

type SetPhase = (id: string, phase: ActivityPhase, source: TransitionSource) => void;

/// Ceiling for background tasks that have no deadline of their own: a
/// Bash/PowerShell task or a persistent/no-timeout Monitor, measured from
/// max(deferral armed, task started). A Monitor with a timeout is ended
/// by `expireDue` at its deadline instead. Marked overdue, not removed:
/// it is presumably still running.
function ceilingOverdueTasks(id: string, armedAt: number, now: number): number {
	const ids: string[] = [];
	for (const e of backgroundTasks.live(id)) {
		if (e.fromAttach || e.overdue || deadlineOf(e) !== null) continue;
		if (now >= Math.max(armedAt, e.startedAt) + BACKGROUND_TASK_CEILING_MS) ids.push(e.taskId);
	}
	if (ids.length === 0) return 0;
	const n = backgroundTasks.markOverdue(id, ids);
	if (n > 0) {
		console.warn(
			`[skein] harness ${id}: ${n} background task(s) held the deferred end of turn for ` +
				`${BACKGROUND_TASK_CEILING_MS / 60_000} min; presumed still running, no longer counted`,
		);
	}
	return n;
}

/// One armed deferral's tick. Returns when done; the caller `continue`s.
function tickDeferred(
	id: string,
	a: HarnessActivity,
	armedAt: number,
	now: number,
	setPhase: SetPhase,
): void {
	// #86: permission is the harder stop. Leave the harness alone;
	// re-evaluate on a later tick once the dialog clears back to `running`.
	if (a.phase === "permission") return;

	let released = ceilingOverdueTasks(id, armedAt, now);
	// Rule 4 — subagent ceiling. See `DELEGATION_CEILING_MS` for the
	// measurement behind the threshold.
	if (subagents.workingCount(id) > 0 && now - a.delegationActivityAt >= DELEGATION_CEILING_MS) {
		const dropped = subagents.presumeGone(id);
		released += dropped;
		console.warn(
			`[skein] harness ${id}: presumed ${dropped} subagent(s) gone after ` +
				`${DELEGATION_CEILING_MS / 60_000} min with no subagent event; flushing the deferred end of turn`,
		);
	}

	if (outstandingWork(id) > 0) {
		// Still non-empty (perhaps again, after having been seen empty):
		// clear the mark so a fresh settle window starts cleanly.
		if (a.delegationEmptiedAt !== null) a.delegationEmptiedAt = null;
		return;
	}
	if (released > 0) {
		// Nothing actually finished, so no settle window: flush now, with
		// the source that tells notifications not to claim completion.
		a.delegationDeferredAt = null;
		a.delegationEmptiedAt = null;
		setPhase(id, "waiting", TRANSITION_SOURCE.DelegationCeiling);
		return;
	}
	// Rule 3 — settle timer. See `DELEGATION_SETTLE_MS` for the
	// measurement behind the threshold.
	if (a.delegationEmptiedAt === null) {
		a.delegationEmptiedAt = now;
	} else if (now - a.delegationEmptiedAt >= DELEGATION_SETTLE_MS) {
		a.delegationDeferredAt = null;
		a.delegationEmptiedAt = null;
		setPhase(id, "waiting", TRANSITION_SOURCE.DelegationSettled);
	}
}

/// #441: a `running` authoritative harness with outstanding work, no
/// deferral armed, and no signal of any kind for the ceiling has lost
/// its end (a dropped `SubagentStop`, a task notification that never
/// landed). Presume the work gone and flip to `waiting` — nothing is
/// claimed finished. Returns true when it fired.
///
/// A foreground tool writes no transcript row until it returns, so a
/// long one reads as silence here. That is safe only because Claude
/// Code caps a foreground Bash/PowerShell call at 10 min, under the
/// 15 min `DELEGATION_CEILING_MS`; a foreground `Agent` call bumps
/// `delegationActivityAt` through its subagent's own rows. Revisit if
/// that cap is ever raised.
function tickStaleWork(id: string, a: HarnessActivity, now: number, setPhase: SetPhase): boolean {
	if (a.phase !== "running" || !a.authoritative || outstandingWork(id) === 0) return false;
	let newestTask = 0;
	for (const e of backgroundTasks.live(id)) {
		if (!e.fromAttach && !e.overdue && e.startedAt > newestTask) newestTask = e.startedAt;
	}
	const lastWorkAt = Math.max(
		a.lastTurnSignal?.at ?? a.spawnedAt,
		a.delegationActivityAt,
		newestTask,
	);
	if (now - lastWorkAt < DELEGATION_CEILING_MS) return false;
	const subs = subagents.presumeGone(id);
	const tasks = backgroundTasks.presumeGone(id);
	console.warn(
		`[skein] harness ${id}: no signal for ${DELEGATION_CEILING_MS / 60_000} min with work outstanding; ` +
			`presumed ${subs} subagent(s) and ${tasks} background task(s) gone (nothing claimed finished)`,
	);
	setPhase(id, "waiting", TRANSITION_SOURCE.WorkWatchdog);
	return true;
}

/// Called by the core tick for EVERY store entry at the top of the loop
/// body. Returns true when the tick must `continue` for this harness.
///
/// A deferred harness is evaluated before the tick's `authoritative`
/// skip because it IS authoritative by definition: only the Claude
/// translator's `awaitingPromptFromAdapter` arms one, and only while an
/// L2c adapter is attached.
export function tickWork(id: string, a: HarnessActivity, now: number, setPhase: SetPhase): boolean {
	// A Monitor ends at its own deadline; the Rust sweep only runs on
	// transcript ticks, so expire here too.
	backgroundTasks.expireDue(id, now);
	if (a.delegationDeferredAt !== null) {
		tickDeferred(id, a, a.delegationDeferredAt, now, setPhase);
		return true;
	}
	return tickStaleWork(id, a, now, setPhase);
}
