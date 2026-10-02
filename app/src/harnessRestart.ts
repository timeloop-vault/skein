// harnessRestart — the pure gate and argv for "restart this harness" (#490).
//
// A restart kills the harness's process and respawns it from its own
// record. It is only safe where nothing is in flight that the kill would
// lose: no turn running, no dialog open, no held mail, no unsent text.
// Every refusal is a reason string a caller can show verbatim as a
// disabled-button tooltip. Pure — no React, no invoke, no store.

import type { ComposerDraft } from "./composerDraft.ts";
import { checkDraft } from "./composerDraft.ts";
import type { HarnessCapabilities } from "./data.tsx";
import { HARNESS_KINDS } from "./data.tsx";
import type { ActivityPhase } from "./harnessActivityTypes.ts";
import { resumeHarness } from "./harnessCmd.ts";
import type { GateResult } from "./harnessInputGate.ts";
import type { Harness, HarnessKind } from "./types.ts";

export interface CanRestartInput {
	kind: HarnessKind;
	capabilities: HarnessCapabilities;
	/// `null` = no activity record: nothing is known to be running.
	phase: ActivityPhase | null;
	/// `mailHold.get(id).held` — mail waiting on the draft guard.
	mailHeld: boolean;
	draft: ComposerDraft;
}

function refuse(reason: string): GateResult {
	return { ok: false, reason };
}

/// Order of checks: capability, phase, mail, draft.
export function canRestart(input: CanRestartInput): GateResult {
	const { kind, capabilities, phase, mailHeld, draft } = input;
	if (!capabilities.pty || !capabilities.resume) {
		return refuse(`${HARNESS_KINDS[kind].name} can't be restarted`);
	}
	switch (phase) {
		case "waiting":
		case "idle":
			break;
		case "running":
			return refuse("busy — wait for its turn to end");
		case "permission":
			return refuse("a permission dialog is open");
		case "spawning":
			return refuse("it is still starting");
		case "exited":
			return refuse("it has already exited — no live process to restart");
		case null:
			return refuse("no live process to restart");
		default: {
			const unreachable: never = phase;
			return refuse(`unknown phase ${String(unreachable)}`);
		}
	}
	if (mailHeld) return refuse("it has held mail waiting to be delivered");
	// `unknown` is refused like `typed` (checkDraft does both): losing a
	// user's draft is the failure this gate exists to avoid, and an
	// unknown draft MAY hold text. A false refusal only costs a retry.
	const held = checkDraft(draft);
	if (held) return refuse("unsent text in the prompt — submit or clear it first");
	return { ok: true };
}

/// The ONE place the respawn record is built: `resumeHarness` returns the
/// whole harness, so a session id minted for a Claude harness with none
/// (#486) comes back on the record for the caller to persist.
export function restartedHarness(
	h: Harness,
	opencodePort?: number,
	mintSessionId?: () => string,
): Harness {
	return resumeHarness(h, opencodePort, mintSessionId);
}
