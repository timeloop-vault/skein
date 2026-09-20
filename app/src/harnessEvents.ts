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
import { TRANSITION_SOURCE, type TransitionSource, harnessActivity } from "./harnessActivity.ts";
import { observedAgents } from "./harnessAgent.ts";
import { type PendingPhase, pendingPrompts } from "./pendingPrompts.ts";
import { subagents } from "./subagents.ts";

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
	| { kind: "session_end" }
	// #298 — Claude subagents (Task tool). A subagent transcript
	// appeared, or one was found still running at attach time
	// (`subagent_start`, idempotent per `agent_id` — see
	// `subagents.record`); it reached a terminal stop reason
	// (`subagent_end`); it got a tool result, proof its own permission
	// gate (if any) is gone (`subagent_tool_result`).
	//
	// `initial` (#277): `true` only for the batch `attach_at` seeds
	// from disk at attach time — see the doc comment on the Rust
	// `SubagentStart` variant. Read defensively as `=== true`, same as
	// every other boolean crossing this boundary.
	| {
			kind: "subagent_start";
			agent_id: string;
			agent_type: string | null;
			description: string | null;
			initial: boolean;
	  }
	| { kind: "subagent_tool_result"; agent_id: string }
	| {
			kind: "subagent_end";
			agent_id: string;
			agent_type: string | null;
			description: string | null;
	  };

/// Subscribe a Claude harness to its JSONL event stream. Marks the
/// activity store as authoritative-source so the L2a idle tick stops
/// fighting the adapter. Returns an unsubscribe — callers should run
/// it from the same cleanup that kills the PTY.
///
/// Soft-fail: if the Rust side rejects `claude_events_attach` (HOME
/// unset, parent dir unwritable, …) we log and fall back to L2a. The
/// harness keeps working, just without the sharper waiting signal.
export function attachClaudeEvents(
	harnessId: string,
	roomId: string,
	sessionId: string,
	cwd: string,
): () => void {
	const channel = new Channel<ClaudeEvent>();
	let closed = false;
	channel.onmessage = (event) => {
		// #259: any event at all proves the tail is on the right file.
		// Guarded so a straggler after unsubscribe can't hand authority
		// back to an adapter that no longer exists.
		if (!closed) harnessActivity.adapterDelivered(harnessId);
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
		roomId,
		sessionId,
		cwd,
		onEvent: channel,
	}).catch((err: unknown) => {
		const msg = err instanceof Error ? err.message : String(err);
		console.warn(`[skein] claude_events_attach failed for ${harnessId}:`, msg);
		harnessActivity.detachAuthoritativeSource(harnessId);
	});

	return () => {
		closed = true;
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
			// Adapter saw the start of a Claude turn. Persisted with
			// `l2c1-claude-assistant` so the activity feed can tell
			// "started thinking" apart from "tool result returned".
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeAssistant);
			return;
		case "tool_use_start":
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeToolUse);
			return;
		case "tool_use_result":
			// #86: a denial writes a tool_result with `is_error` set,
			// same shape as any other result — verified against real
			// Claude Code sessions. Either way the permission gate this
			// tool call was waiting on is gone, approved or not.
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeToolResult, {
				clearsPermission: true,
			});
			return;
		case "user_prompt":
			// #86: the user submitted a fresh prompt — whatever
			// permission dialog might have been showing is gone by
			// construction (Claude doesn't accept a new prompt while
			// one is up).
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeUserPrompt, {
				clearsPermission: true,
			});
			// #277: a fresh prompt starts a new delegation-counting
			// window.
			harnessActivity.noteUserPrompt(harnessId);
			return;
		case "awaiting_prompt":
			// The signal we built this for: an assistant row with a
			// terminal stop_reason (end_turn / stop_sequence /
			// max_tokens) → "I'm done, awaiting your next prompt."
			// #260: also an interrupt row or a turn_duration row — an
			// interrupted turn never gets a terminal stop_reason.
			//
			// #277: routed through `awaitingPromptFromAdapter`, not
			// `setWaitingFromAdapter` directly — the main transcript's
			// own end of turn is a lie about the harness being done
			// while subagents it just delegated to are still working.
			harnessActivity.awaitingPromptFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
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
			// reflects PTY truth. #86: a dialog open at this moment
			// would otherwise pin `permission` for good — the L2a tick
			// never touches a non-running phase, and no tool_result can
			// arrive from a file that is gone.
			harnessActivity.releasePermission(harnessId, TRANSITION_SOURCE.AdapterDetached);
			harnessActivity.detachAuthoritativeSource(harnessId);
			return;
		// #298: subagent bookkeeping — none of these three may call
		// `setRunningFromAdapter` or `setWaitingFromAdapter` directly.
		// The parent (main-session) PHASE is deliberately left untouched
		// here: a subagent starting, finishing, or getting a tool result
		// says nothing about whether the main session is running,
		// waiting, or idle, and guessing wrongly would fight whatever
		// the main transcript's own events are already saying. #277
		// adds bookkeeping (`note*`) alongside these that feeds the
		// deferred-end-of-turn timers, but still no direct phase write.
		case "subagent_start":
			subagents.record(
				harnessId,
				{ agentId: event.agent_id, agentType: event.agent_type, description: event.description },
				event.initial,
			);
			// #277: only a LIVE start counts toward `delegatedCount` and
			// counts as proof-of-life for the ceiling — an attach-time
			// replay belongs to a PTY that's already dead (see
			// `SubagentEntry.fromAttach`) and must not look like fresh
			// activity.
			if (event.initial !== true) harnessActivity.noteSubagentStarted(harnessId);
			return;
		case "subagent_end":
			subagents.finish(harnessId, event.agent_id);
			harnessActivity.noteSubagentActivity(harnessId);
			return;
		case "subagent_tool_result":
			// The one narrow phase effect a subagent's own tool result is
			// allowed: prove its own permission gate is gone — not a
			// different subagent's, which `event.agent_id` lets
			// `clearPermission` tell apart. See
			// `harnessActivity.clearPermission`.
			harnessActivity.clearPermission(
				harnessId,
				TRANSITION_SOURCE.L2c1ClaudeSubagentToolResult,
				event.agent_id,
			);
			// #277: proof of life either way, phase effect or not.
			harnessActivity.noteSubagentActivity(harnessId);
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
	| { kind: "user_message_agent"; session_id: string; agent: string }
	// #86 — opencode's own permission and question prompts. Not
	// filtered by session: one opencode process backs the whole
	// harness, and a subagent child session's prompt blocks the user
	// just as much as the top-level session's would.
	| { kind: "permission_asked"; request_id: string; session_id: string }
	| { kind: "permission_replied"; request_id: string; session_id: string }
	| { kind: "question_asked"; request_id: string; session_id: string }
	| { kind: "question_resolved"; request_id: string; session_id: string }
	| { kind: "session_end" };

/// Subscribe an opencode harness to its embedded-server SSE stream
/// `cwd` is the room worktree — the backend needs it to capture a
/// review baseline for each file the harness writes (#211).
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
	roomId: string,
	cwd: string,
	port: number,
	sessionId: string | undefined,
	onSessionCaptured: ((sessionId: string) => void) | undefined,
): () => void {
	const channel = new Channel<OpencodeEvent>();
	let closed = false;
	channel.onmessage = (event) => {
		// #259: see attachClaudeEvents. `connected` arrives first, so a
		// stream that is up disarms the watchdog before any prompt.
		if (!closed) harnessActivity.adapterDelivered(harnessId);
		translateOpencode(harnessId, event, onSessionCaptured);
	};

	// See attachClaudeEvents for why this happens synchronously.
	harnessActivity.attachAuthoritativeSource(harnessId);

	void invoke("opencode_events_attach", {
		harnessId,
		roomId,
		cwd,
		port,
		sessionId: sessionId ?? null,
		onEvent: channel,
	}).catch((err: unknown) => {
		const msg = err instanceof Error ? err.message : String(err);
		console.warn(`[skein] opencode_events_attach failed for ${harnessId}:`, msg);
		harnessActivity.detachAuthoritativeSource(harnessId);
	});

	return () => {
		closed = true;
		harnessActivity.detachAuthoritativeSource(harnessId);
		// The process this was observed on is going away (#248).
		observedAgents.forget(harnessId);
		// #86: outstanding permission/question ids are meaningless once
		// this adapter is gone — nothing will ever resolve them.
		pendingPrompts.forget(harnessId);
		void invoke("opencode_events_detach", { harnessId }).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn(`[skein] opencode_events_detach failed for ${harnessId}:`, msg);
		});
	};
}

/// Apply a `PendingPhase` derived from `pendingPrompts` to the
/// activity store. `permission` re-asserts (idempotent if already
/// there); `waiting`/`running` both need `clearsPermission: true` on
/// the running arm since draining every pending prompt is exactly the
/// signal that means the gate is gone. #86.
const applyPendingPhase = (
	harnessId: string,
	phase: PendingPhase,
	source: TransitionSource,
): void => {
	switch (phase) {
		case "permission":
			harnessActivity.setPermissionFromAdapter(harnessId, source, null);
			return;
		case "waiting":
			harnessActivity.setWaitingFromAdapter(harnessId, source);
			return;
		case "running":
			harnessActivity.setRunningFromAdapter(harnessId, source, { clearsPermission: true });
			return;
	}
};

const translateOpencode = (
	harnessId: string,
	event: OpencodeEvent,
	onSessionCaptured: ((sessionId: string) => void) | undefined,
): void => {
	switch (event.kind) {
		case "connected":
			// Re-arm authoritative on every successful (re)connect.
			// First connect: synchronous `attachAuthoritativeSource`
			// in `attachOpencodeEvents` already armed it; this is
			// a no-op. Reconnect: SessionEnd previously detached
			// authoritative so L2a could take over during the
			// outage — now that we're back on the wire, we want
			// adapter phases to win again.
			//
			// Phase change isn't done here; the synthetic
			// SessionIdle the Rust adapter emits right after
			// Connected handles the baseline state (see
			// `stream_events` for the rationale).
			//
			// #86: a reconnect replays nothing, so any permission/
			// question ids we were tracking are gone for good — the
			// synthetic SessionIdle that follows will put the harness
			// in `waiting`, not leave it stuck in `permission` forever.
			pendingPrompts.forget(harnessId);
			harnessActivity.attachAuthoritativeSource(harnessId);
			return;
		case "session_created":
			// SSE-driven session-id capture (replaces chapter 5
			// phase 2b's sqlite poll on the happy path). The
			// callback short-circuits the sqlite fallback once
			// invoked.
			onSessionCaptured?.(event.session_id);
			return;
		case "session_busy":
			// #86: "still working" — must not clear a pending
			// permission, so no `clearsPermission` (defaults to false).
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c2OpencodeBusy);
			return;
		case "message_delta":
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c2OpencodeMessageDelta);
			return;
		case "tool_use_start":
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c2OpencodeToolUse);
			return;
		case "user_message_agent":
			// Not a phase signal. Recorded with its session so the label
			// can ignore a subagent's child session (see `agentLabel`).
			observedAgents.record(harnessId, event.session_id, event.agent);
			return;
		case "permission_asked":
			pendingPrompts.permissionAsked(harnessId, event.request_id);
			harnessActivity.setPermissionFromAdapter(
				harnessId,
				TRANSITION_SOURCE.L2c2OpencodePermission,
				null,
			);
			return;
		case "permission_replied":
			applyPendingPhase(
				harnessId,
				pendingPrompts.permissionReplied(harnessId, event.request_id),
				TRANSITION_SOURCE.L2c2OpencodePermissionReplied,
			);
			return;
		case "question_asked":
			// Outranked by a pending permission — `applyPendingPhase`
			// re-asserts `permission` rather than overwriting it with
			// `waiting` in that case.
			applyPendingPhase(
				harnessId,
				pendingPrompts.questionAsked(harnessId, event.request_id),
				TRANSITION_SOURCE.L2c2OpencodeQuestion,
			);
			return;
		case "question_resolved":
			applyPendingPhase(
				harnessId,
				pendingPrompts.questionResolved(harnessId, event.request_id),
				TRANSITION_SOURCE.L2c2OpencodeQuestionResolved,
			);
			return;
		case "session_idle":
			// The signal we built this for: opencode finished its
			// turn and is awaiting user input. Unconditional — #86:
			// whatever was pending is moot once the turn itself ends.
			pendingPrompts.forget(harnessId);
			harnessActivity.setWaitingFromAdapter(harnessId, TRANSITION_SOURCE.L2c2OpencodeIdle);
			return;
		case "session_end":
			// Server disconnected after a successful connect. Drop
			// authoritative so L2a takes over until the Rust side
			// reconnects (it keeps trying with backoff while the
			// PTY lives). #86: also drop any pending permission/
			// question ids — nothing will resolve them across the gap.
			pendingPrompts.forget(harnessId);
			harnessActivity.releasePermission(harnessId, TRANSITION_SOURCE.AdapterDetached);
			harnessActivity.detachAuthoritativeSource(harnessId);
			return;
	}
};
