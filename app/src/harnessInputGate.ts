// harnessInputGate — the gate result type and the purely local checks over
// the #238 nudge seam.
//
// "No nudge where safety can't be proven": every refusal comes back as a
// reason string a caller can show verbatim as a disabled-button tooltip or
// a drag-overlay label, never a silent no-op.
//
// Since #733 the DAEMON decides whether text may go into a harness
// (`harness_gate_check`, see `gateCheck.ts`). What stays here is what only
// the webview can know: the kind has a terminal, and that terminal is
// registered in the seam.

import type { HarnessCapabilities } from "./data.tsx";
import type { HarnessActivity } from "./harnessActivity.ts";

/// `draftHeld` (#413) is set only by the #383 composer-draft guard. `code`
/// is the daemon's camelCase refusal code, when the refusal came from it.
export type GateResult =
	| { ok: true }
	| { ok: false; reason: string; draftHeld?: true; code?: string };

/// A kind with no `pty` capability (`files`), a harness that hasn't
/// registered yet (just spawned, or just exited), or one with no activity
/// record at all (same two cases, seen from the store side) have no
/// terminal the seam can paste into. `null` = nothing local objects.
export function checkRegistered(input: {
	capabilities: HarnessCapabilities;
	activity: HarnessActivity | null;
	registered: boolean;
}): GateResult | null {
	if (!input.capabilities.pty) {
		return { ok: false, reason: "this harness has no terminal to paste into" };
	}
	if (!input.registered || !input.activity) {
		return { ok: false, reason: "this harness's terminal isn't ready yet" };
	}
	return null;
}

/// Wrap every path in `paths` that contains whitespace in double quotes,
/// join with single spaces, and add one trailing space so the user can
/// keep typing right after the drop lands. `[]` returns `""` — nothing
/// to insert, nothing to seed the input with.
export function formatDroppedPaths(paths: string[]): string {
	if (paths.length === 0) return "";
	const quoted = paths.map((p) => (/\s/.test(p) ? `"${p}"` : p));
	return `${quoted.join(" ")} `;
}
