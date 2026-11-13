// Client-only phase inputs, forwarded to the daemon (#732). The daemon's
// Rust phase machine runs in shadow beside the TypeScript one and cannot
// see these facts (they exist only in the webview). Fire-and-forget: the
// TypeScript machine never waits on, or depends on, the answer.

import { invoke } from "@tauri-apps/api/core";

export type PhaseInput =
	| { kind: "input"; data: string; emptyComposer: boolean }
	| { kind: "promptSubmitted" }
	| { kind: "muteInducedOutput" }
	| { kind: "screenReading"; reading: "waiting" | "running" | "unknown" | null }
	| { kind: "hermesLaunchPoll"; screenWaiting: boolean }
	| { kind: "launchSignal" }
	| { kind: "sessionSwitched"; switch: "clear" | "resume" | "fork" | "startup" }
	| { kind: "recoverUnheardSilence" }
	| { kind: "reattached" }
	| { kind: "viewReset" };

export function sendPhaseInput(harnessId: string, input: PhaseInput): void {
	try {
		void invoke("harness_phase_input", { harnessId, input }).catch((e: unknown) => {
			console.debug("[skein] harness_phase_input failed", e);
		});
	} catch (e) {
		console.debug("[skein] harness_phase_input failed", e);
	}
}

/// A page that did not start the process (`ensureRunning` reported
/// `started: false`) is rebuilding a phase record for a harness that kept
/// running: a reload cannot tell the daemon, so the first attach does. Sent
/// before this page's adapters or `reattached` can feed the machine.
export function noteViewAttached(harnessId: string, started: boolean): void {
	if (!started) sendPhaseInput(harnessId, { kind: "viewReset" });
}
