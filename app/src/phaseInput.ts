// Client-only phase inputs, forwarded to the daemon (#732). The daemon's
// phase machine is the authority but cannot see these facts (they exist only
// in the webview). Fire-and-forget: the mirror never waits on the answer.

import { invoke } from "@tauri-apps/api/core";

export type RestartSource = "user-restart" | "version-auto-restart";

export type PhaseInput =
	| { kind: "input"; data: string; emptyComposer: boolean }
	| { kind: "promptSubmitted" }
	| { kind: "muteInducedOutput" }
	| { kind: "screenReading"; reading: "waiting" | "running" | "unknown" | null }
	| { kind: "hermesLaunchPoll"; screenWaiting: boolean }
	| { kind: "launchSignal" }
	| { kind: "sessionSwitched"; switch: "clear" | "resume" | "fork" | "startup" }
	| { kind: "recoverUnheardSilence" }
	/// A start produced no process (agent refused, the daemon returned no child).
	| { kind: "startFailed" }
	/// The user (or the version auto-restart) asked to restart the harness.
	| { kind: "restartRequested"; source: RestartSource };

export function sendPhaseInput(harnessId: string, input: PhaseInput): void {
	try {
		void invoke("harness_phase_input", { harnessId, input }).catch((e: unknown) => {
			console.debug("[skein] harness_phase_input failed", e);
		});
	} catch (e) {
		console.debug("[skein] harness_phase_input failed", e);
	}
}
