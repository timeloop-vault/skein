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

// ── opencode (L2c-2) ───────────────────────────────────────────────

/// Mirror of the Rust `OpencodeEvent` enum in
/// `harness_events_opencode.rs`. Keep in lock-step; unknown event
/// kinds added later are treated as no-ops by the translator until
/// the frontend catches up.
export type OpencodeEvent =
	| { kind: "connected" }
	| { kind: "session_created"; session_id: string }
	| { kind: "session_busy" }
	| { kind: "session_idle" }
	| { kind: "message_delta" }
	| { kind: "tool_use_start"; name: string }
	| { kind: "session_end" };

/// Subscribe an opencode harness to its embedded-server SSE stream
/// on `127.0.0.1:<port>`. Synchronously marks the activity store as
/// authoritative-source (same dance as Claude — see comment in
/// `attachClaudeEvents`). Returns an unsubscribe.
///
/// `onSessionCaptured` fires when the SSE stream delivers a
/// `session.created` event with its sessionID. The caller (App)
/// wires this to `setHarnessSessionId` so the captured id becomes
/// the resume target on next Skein restart. Chapter 5's sqlite
/// poll stays as a fallback — see `captureOpencodeSessionId` for
/// the relationship.
///
/// Soft-fail: any Rust-side rejection (port closed, IPC dropped)
/// detaches authoritative + warns. The harness keeps running on
/// L2a.
export function attachOpencodeEvents(
	harnessId: string,
	port: number,
	onSessionCaptured: ((sessionId: string) => void) | undefined,
): () => void {
	const channel = new Channel<OpencodeEvent>();
	channel.onmessage = (event) => {
		translateOpencode(harnessId, event, onSessionCaptured);
	};

	// See attachClaudeEvents for why this happens synchronously.
	harnessActivity.attachAuthoritativeSource(harnessId);

	void invoke("opencode_events_attach", { harnessId, port, onEvent: channel }).catch(
		(err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn(`[skein] opencode_events_attach failed for ${harnessId}:`, msg);
			harnessActivity.detachAuthoritativeSource(harnessId);
		},
	);

	return () => {
		harnessActivity.detachAuthoritativeSource(harnessId);
		void invoke("opencode_events_detach", { harnessId }).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn(`[skein] opencode_events_detach failed for ${harnessId}:`, msg);
		});
	};
}

const translateOpencode = (
	harnessId: string,
	event: OpencodeEvent,
	onSessionCaptured: ((sessionId: string) => void) | undefined,
): void => {
	switch (event.kind) {
		case "connected":
			// Server reachable. No phase change — we don't know yet
			// whether opencode is busy or idle until the first
			// session.status event arrives. The mere fact of
			// attachment is signal to the Rust adapter (reset
			// backoff); the frontend has nothing to do.
			return;
		case "session_created":
			// SSE-driven session-id capture (replaces chapter 5
			// phase 2b's sqlite poll on the happy path). The
			// callback short-circuits the sqlite fallback once
			// invoked.
			onSessionCaptured?.(event.session_id);
			return;
		case "session_busy":
		case "message_delta":
		case "tool_use_start":
			harnessActivity.setRunningFromAdapter(harnessId);
			return;
		case "session_idle":
			// The signal we built this for: opencode finished its
			// turn and is awaiting user input.
			harnessActivity.setWaitingFromAdapter(harnessId);
			return;
		case "session_end":
			// Server disconnected after a successful connect. Drop
			// authoritative so L2a takes over until the Rust side
			// reconnects (it keeps trying with backoff while the
			// PTY lives).
			harnessActivity.detachAuthoritativeSource(harnessId);
			return;
	}
};
