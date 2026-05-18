// Harness-native event adapters — frontend translator. Epic #50 L2c.
//
// Rust-side adapters (today: `harness_events_claude.rs`) emit semantic
// events over a Tauri Channel. This module subscribes, marks the
// harness as having an authoritative source so the L2a idle tick
// stands down, and translates each event into a phase call on
// `harnessActivity`.
//
// Why keep the policy here and not in Rust: the state machine and
// every consumer of it (status bar, badges, OS notifications) lives
// in the frontend. Translating event → phase in Rust would split the
// policy across the boundary; keeping it here means the Rust adapter
// is a pure "what did Claude write to its log" producer, and the
// "what does it mean for the dot" logic stays next to everything else
// that reads from the store.

import { Channel, invoke } from "@tauri-apps/api/core";
import { harnessActivity } from "./harnessActivity.ts";

/// Mirror of the Rust enum. `kind` is the serde tag from
/// `harness_events_claude.rs`'s `ClaudeEvent`. Keep these in lock-step;
/// new event types added on the Rust side fall into the `default`
/// branch of the translator switch and are ignored harmlessly until
/// the policy here catches up.
export type ClaudeEvent =
	| { kind: "assistant_turn" }
	| { kind: "tool_use_start"; name: string }
	| { kind: "tool_use_result" }
	| { kind: "user_prompt" }
	| { kind: "awaiting_prompt" }
	| { kind: "attachment" }
	| { kind: "session_end" };

/// Subscribe a Claude harness to its JSONL event stream. Marks the
/// activity store as authoritative-source so the L2a idle tick stops
/// fighting the adapter. Returns an unsubscribe — callers should run
/// it from the same cleanup that kills the PTY.
///
/// Soft-fail: if the Rust side rejects `claude_events_attach` (HOME
/// unset, parent dir unwritable, …) we log and fall back to L2a. The
/// harness keeps working, just without the sharper waiting signal.
export function attachClaudeEvents(harnessId: string, sessionId: string, cwd: string): () => void {
	const channel = new Channel<ClaudeEvent>();
	channel.onmessage = (event) => {
		translate(harnessId, event);
	};

	// Mark authoritative *synchronously*, not on `.then()`. By the
	// time attachClaudeEvents is called, the PTY has been streaming
	// chunks for some time (LiveTerminal calls us after pty_spawn
	// resolves). If we wait for the rust attach to round-trip
	// before flipping authoritative, those in-flight chunks call
	// recordOutput → setPhase("running") and override whatever the
	// adapter's history probe is about to emit. Net result: harness
	// stays green even though it should be waiting. Found in
	// v0.1.9-dev. Detach in the catch handler if the rust side
	// rejects, so we fall back to L2a cleanly.
	harnessActivity.attachAuthoritativeSource(harnessId);

	void invoke("claude_events_attach", {
		harnessId,
		sessionId,
		cwd,
		onEvent: channel,
	}).catch((err: unknown) => {
		const msg = err instanceof Error ? err.message : String(err);
		console.warn(`[skein] claude_events_attach failed for ${harnessId}:`, msg);
		harnessActivity.detachAuthoritativeSource(harnessId);
	});

	return () => {
		harnessActivity.detachAuthoritativeSource(harnessId);
		void invoke("claude_events_detach", { harnessId }).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn(`[skein] claude_events_detach failed for ${harnessId}:`, msg);
		});
	};
}

const translate = (harnessId: string, event: ClaudeEvent): void => {
	switch (event.kind) {
		case "assistant_turn":
		case "tool_use_start":
		case "tool_use_result":
		case "user_prompt":
			// All four mean the harness is on a turn / processing
			// input. The state machine just needs `running`; the
			// richer events (which tool, etc.) get surfaced by L7
			// when the activity feed lands.
			harnessActivity.setRunningFromAdapter(harnessId);
			return;
		case "awaiting_prompt":
			// The signal we built this for: an assistant row with a
			// terminal stop_reason (end_turn / stop_sequence /
			// max_tokens) → "I'm done, awaiting your next prompt."
			harnessActivity.setWaitingFromAdapter(harnessId);
			return;
		case "attachment":
			// User-side action between turns — doesn't shift phase
			// (the harness is still effectively waiting until the
			// user submits). No-op.
			return;
		case "session_end":
			// File vanished. Fall back to L2a — chunk-driven idle
			// detection takes over. The Rust adapter stays alive in
			// case the file reappears, but until then the dot
			// reflects PTY truth.
			harnessActivity.detachAuthoritativeSource(harnessId);
			return;
	}
};
