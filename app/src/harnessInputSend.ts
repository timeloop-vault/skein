// harnessInputSend — what the webview still does with the #238 seam.
//
// Prompt delivery moved to the daemon (#733): `sendPrompt` only asks it to
// (`harness_send_prompt`), and the daemon gates, pastes, waits, presses
// Enter and retries. No code in the webview writes a submit for a prompt.
// `insertText` stays here: a file drop is a user action that only pastes,
// through the registered terminal.

import { invoke } from "@tauri-apps/api/core";
import { HARNESS_KINDS } from "./data.tsx";
import { harnessActivity } from "./harnessActivity.ts";
import type { GateResult } from "./harnessInputGate.ts";
import { canInsertText } from "./harnessInputGate.ts";
import { harnessInput } from "./harnessInputRegistry.ts";
import { mapDeliveryError, mapDeliveryResult } from "./promptDelivery.ts";
import type { HarnessKind } from "./types.ts";

/// Ask the daemon to send `body` to `harnessId` as a manual prompt (the
/// review Nudge button, a prompt-library item). The daemon re-checks its own
/// gate; the `canSendPrompt` a caller rendered is only a display hint.
export async function sendPrompt(harnessId: string, body: string): Promise<GateResult> {
	try {
		return mapDeliveryResult(await invoke("harness_send_prompt", { harnessId, body }));
	} catch (err) {
		return mapDeliveryError(err);
	}
}

/// The held-mail "deliver now" click: ask the daemon to deliver the held
/// mail now. Same answer shape as `sendPrompt`.
export async function deliverMailNow(harnessId: string): Promise<GateResult> {
	try {
		return mapDeliveryResult(await invoke("mail_deliver_now", { harnessId }));
	} catch (err) {
		return mapDeliveryError(err);
	}
}

/// Paste `text` into `harnessId`'s terminal — and only that, never
/// `submit` — after re-checking `canInsertText` against the *current*
/// state. #41's drop-a-file caller: a path dropped mid-turn seeds the
/// input for whatever the user types next, it does not send anything on
/// the harness's behalf.
export function insertText(harnessId: string, kind: HarnessKind, text: string): GateResult {
	const target = harnessInput.target(harnessId);
	const gate = canInsertText({
		capabilities: HARNESS_KINDS[kind].capabilities,
		activity: harnessActivity.get(harnessId),
		registered: target !== undefined,
	});
	if (!gate.ok) return gate;
	// A passing gate required `registered: target !== undefined`, so
	// `target` is set here by construction.
	target?.paste(text);
	return gate;
}
