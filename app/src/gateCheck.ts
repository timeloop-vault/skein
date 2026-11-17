// gateCheck — ask the daemon whether text may go into a harness now (#733 B).
//
// The daemon owns the send gate (`prompt_delivery/gate.rs`); the webview no
// longer re-derives it from the mirrored phase. `harness_gate_check` answers
// the same question the real send/insert would, so a tooltip or a file drop
// can use it without a second copy of the rules.

import { invoke } from "@tauri-apps/api/core";
import type { GateResult } from "./harnessInputGate.ts";

export type GateMode = "send" | "insert";

type GateWire = { ok: true } | { ok: false; code: string; reason: string };

/// `send`: may `body` be pasted and submitted now. `insert`: may text be
/// pasted without submit now (`body` null). If the invoke itself fails, a
/// `send` answer is a display hint only (the daemon still decides at send
/// time), so it reads as allowed; an `insert` has no later check, so it
/// refuses.
export async function checkGate(
	harnessId: string,
	body: string | null,
	mode: GateMode,
): Promise<GateResult> {
	try {
		const wire = await invoke<GateWire>("harness_gate_check", { harnessId, body, mode });
		return wire.ok ? { ok: true } : { ok: false, reason: wire.reason, code: wire.code };
	} catch {
		return mode === "send"
			? { ok: true }
			: { ok: false, reason: "couldn't check whether this harness can take input right now" };
	}
}
