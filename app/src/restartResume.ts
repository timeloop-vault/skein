// The daemon half of a restart (#490, #730): drop a vanished Claude id, ask
// the daemon for the resume argv (and, for opencode, a fresh port), and turn
// either failure into the refusal the restart caller surfaces. Split out of
// useHarnessActions.ts; the daemon call is injected so it is testable.

import type { ResumedHarness } from "./harnessArgvApi.ts";
import type { GateResult } from "./harnessInputGate.ts";
import type { Harness } from "./types.ts";

export type RestartPrep =
	| { ok: true; resumed: ResumedHarness; port: number | null }
	| Extract<GateResult, { ok: false }>;

/** `pre` is the record re-read after the awaits. When `dropId` is set (its
 *  session is gone and `/clear` etc. did not change it meanwhile) the id is
 *  removed so the daemon mints a fresh one; cmd and sessionId persist
 *  together so the telemetry tail follows the new id. The opencode SSE
 *  adapter reads its port from the `opencodePorts` map, not from the argv. */
export async function prepareRestart(
	pre: Harness,
	dropId: boolean,
	resumeCmd: (h: Harness) => Promise<ResumedHarness>,
): Promise<RestartPrep> {
	const { sessionId: _stale, ...noId } = pre;
	let resumed: ResumedHarness;
	try {
		resumed = await resumeCmd(dropId ? noId : pre);
	} catch (err) {
		console.warn("[skein] harness_resume_cmd failed; not restarting:", err);
		return { ok: false, reason: "couldn't prepare the restarted harness" };
	}
	if (pre.kind === "opencode" && resumed.opencodePort === null) {
		console.warn("[skein] opencode port allocation failed; not restarting");
		return { ok: false, reason: "couldn't allocate a port for the restarted harness" };
	}
	return { ok: true, resumed, port: resumed.opencodePort };
}
