// The webview's side of the daemon's prompt delivery (#733): the wire shape
// of `harness_send_prompt` / `mail_deliver_now` and its mapping onto the
// `GateResult` the nudge button, the actions menu and the deliver-now click
// already consume. Pure; the invoke wrappers are in `harnessInputSend.ts`.
// The daemon gates, pastes, waits, presses Enter and retries — nothing here
// touches a terminal.

import type { GateResult } from "./harnessInputGate.ts";

/// `{ ok: true } | { ok: false, code, reason }`, as both commands answer.
export type DeliveryResultDto = { ok: true } | { ok: false; code: string; reason: string };

/// A refusal the daemon gave without a usable reason.
export const UNKNOWN_REFUSAL = "the prompt was not delivered";

/// Map the commands' answer. Anything that is not a recognisable
/// `{ ok: true }` is a refusal, never a silent success.
export function mapDeliveryResult(raw: unknown): GateResult {
	if (typeof raw === "object" && raw !== null) {
		const r = raw as { ok?: unknown; reason?: unknown };
		if (r.ok === true) return { ok: true };
		if (r.ok === false) {
			return {
				ok: false,
				reason: typeof r.reason === "string" && r.reason !== "" ? r.reason : UNKNOWN_REFUSAL,
			};
		}
	}
	return { ok: false, reason: UNKNOWN_REFUSAL };
}

/// A rejected invoke (daemon unreachable, command missing) is a refusal too.
export function mapDeliveryError(err: unknown): GateResult {
	const text = err instanceof Error ? err.message : String(err);
	return { ok: false, reason: text === "" ? UNKNOWN_REFUSAL : text };
}
