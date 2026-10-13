// The activity tick's screen-phase rule (#623): a TUI that never goes
// byte-quiet (Hermes redraws a clock and a spinner constantly) can't reach
// L2a idle or an L2b tail match, so its running <-> waiting comes from the
// rendered screen. Kept out of harnessActivityCore.ts for file size; the
// core hands in its own `setPhase`, the way deferral.ts does.

import type { ActivityPhase, HarnessActivity, TransitionSource } from "./harnessActivityTypes.ts";
import { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
import type { ScreenPhase } from "./screenPhase.ts";

type SetPhase = (id: string, phase: ActivityPhase, source: TransitionSource) => void;

/// Consecutive identical readings needed before a transition: a torn
/// ConPTY frame must not flip the phase.
const CONFIRM_READS = 2;

/// A screen-owned waiting whose screen then stops being recognised (an
/// overlay, a picker, a resize) for this many ticks in a row is released to
/// `running`, so the ordinary L2a/L2b rules can take over. Without it the
/// phase would stay stuck at waiting while the agent works.
export const SCREEN_UNKNOWN_RELEASE_TICKS = 5;

/// Injected rather than imported: the input registry owns the terminals,
/// and the activity store must not depend on it. The registry sets this
/// once at module load; unset reads as "unknown".
let reader: ((id: string) => ScreenPhase) | null = null;

export function setScreenPhaseReader(fn: ((id: string) => ScreenPhase) | null): void {
	reader = fn;
}

interface Debounce {
	reading: ScreenPhase;
	count: number;
	/// The activity phase this count was taken in; a change resets it.
	phase: ActivityPhase;
}

const debounce = new Map<string, Debounce>();
/// `phaseSince` of the waiting this module set, per harness.
const screenWaitingSince = new Map<string, number>();
/// Consecutive unrecognised ticks while the screen owns a waiting.
const unknownTicks = new Map<string, number>();

/// Is `a` in a `waiting` that the screen rule set (and still is that same
/// waiting)? Output must not undo it; only the tick may.
export function isScreenWaiting(id: string, a: HarnessActivity): boolean {
	return a.phase === "waiting" && screenWaitingSince.get(id) === a.phaseSince;
}

/// Drop the screen's claim on `id`'s waiting and its debounce state.
export function clearScreenOwnership(id: string): void {
	debounce.delete(id);
	screenWaitingSince.delete(id);
	unknownTicks.delete(id);
}

export const forgetScreenPhase = clearScreenOwnership;

/// One non-authoritative harness's tick. Returns true when the screen gave
/// a positive reading and the caller should skip L2a/L2b for this tick.
export function tickScreenPhase(
	id: string,
	a: HarnessActivity,
	setPhase: SetPhase,
	get: (id: string) => HarnessActivity | undefined,
): boolean {
	if (reader === null) return false;
	const screenOwnsWaiting = isScreenWaiting(id, a);
	if (a.phase !== "running" && !screenOwnsWaiting) {
		debounce.delete(id);
		return false;
	}
	let reading: ScreenPhase;
	try {
		reading = reader(id);
	} catch {
		reading = "unknown";
	}
	if (reading === "unknown") {
		debounce.delete(id);
		if (!screenOwnsWaiting) return false;
		const n = (unknownTicks.get(id) ?? 0) + 1;
		if (n < SCREEN_UNKNOWN_RELEASE_TICKS) {
			unknownTicks.set(id, n);
			return true;
		}
		clearScreenOwnership(id);
		setPhase(id, "running", TRANSITION_SOURCE.L2bScreenCleared);
		return true;
	}
	unknownTicks.delete(id);
	const prev = debounce.get(id);
	const count = prev?.reading === reading && prev.phase === a.phase ? prev.count + 1 : 1;
	debounce.set(id, { reading, count, phase: a.phase });
	if (count < CONFIRM_READS) return true;
	if (a.phase === "running" && reading === "waiting") {
		setPhase(id, "waiting", TRANSITION_SOURCE.L2bScreen);
		// `setPhase` replaces the record, so re-read it for the new phaseSince.
		const next = get(id);
		if (next?.phase === "waiting") screenWaitingSince.set(id, next.phaseSince);
		debounce.delete(id);
	} else if (screenOwnsWaiting && reading === "running") {
		setPhase(id, "running", TRANSITION_SOURCE.L2bScreenCleared);
		clearScreenOwnership(id);
	}
	return true;
}
