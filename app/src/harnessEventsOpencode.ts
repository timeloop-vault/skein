// opencode half of the L2c translator (epic #50, L2c-2): subscribes to
// the Rust `OpencodeEvent` channel (embedded-server SSE, fed by
// `harness_events_opencode.rs`) and translates each event into a phase
// call on `harnessActivity`. Public entry is `harnessEvents.ts`.

import { Channel, invoke } from "@tauri-apps/api/core";
import { TRANSITION_SOURCE, type TransitionSource, harnessActivity } from "./harnessActivity.ts";
import { observedAgents } from "./harnessAgent.ts";
import { beginAttach, endAttach, guardChannelHandler } from "./harnessEventsShared.ts";
import { type PendingPhase, pendingPrompts } from "./pendingPrompts.ts";
import { followedOpencodeSession } from "./sessionTracking.ts";

/// Mirror of the Rust `OpencodeEvent` enum in
/// `harness_events_opencode.rs`. Keep in lock-step; unknown event
/// kinds added later are treated as no-ops by the translator until
/// the frontend catches up.
export type OpencodeEvent =
	| { kind: "connected" }
	// #116: `parent_id` is non-null for a subagent child session — never
	// a conversation to capture or follow. `null` marks a root session.
	| { kind: "session_created"; session_id: string; parent_id: string | null }
	// #116: a user message landed in a ROOT session (backend-verified
	// not a child) — the only signal the `/sessions` picker gives when
	// it switches onto an EXISTING session, since it publishes nothing
	// of its own. See `followedOpencodeSession` in `sessionTracking.ts`.
	| { kind: "root_session_prompted"; session_id: string }
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
/// `getSessionId` reads the harness's CURRENT session id, live, at
/// the moment each event is translated — not the value captured when
/// this adapter attached. The adapter is attached once per PTY, but
/// the harness's own sessionId can change under it (#116: `/new` or a
/// `/sessions` pick), so a value closed over at attach time would go
/// stale the first time that happens.
///
/// `onSessionFollowed` fires when `followedOpencodeSession` decides
/// the harness moved onto a different root session (`/new` or an
/// existing session picked via `/sessions`) — see that function for
/// the two signals this is built from. The caller wires it to
/// `replaceHarnessSessionId`, same as Claude's `/clear`/`/resume`/fork
/// follow. Deliberately NOT routed through `harnessActivity` here: a
/// "switch" is detected by a user prompt arriving, and opencode's own
/// busy/idle SSE (port-scoped, already authoritative) is what should
/// keep driving the phase.
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
	getSessionId: () => string | undefined,
	onSessionFollowed: ((sessionId: string) => void) | undefined,
): () => void {
	const channel = new Channel<OpencodeEvent>();
	let closed = false;
	const { token, isLive } = beginAttach(harnessId, { kind: "opencode" });
	channel.onmessage = guardChannelHandler(harnessId, "opencode_events", (event) => {
		// #259: see attachClaudeEvents. `connected` arrives first, so a
		// stream that is up disarms the watchdog before any prompt.
		// #422: a straggler from a closed or replaced attach must not
		// translate (its `session_end` would detach the live one).
		if (closed || !isLive()) return;
		// `connected` re-arms authority itself, explicitly.
		harnessActivity.adapterDelivered(harnessId, {
			restoresAuthority: event.kind !== "session_end" && event.kind !== "connected",
		});
		translateOpencode(harnessId, event, onSessionCaptured, getSessionId, onSessionFollowed);
	});

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
		if (isLive()) harnessActivity.detachAuthoritativeSource(harnessId);
	});

	return () => {
		closed = true;
		if (isLive()) harnessActivity.detachAuthoritativeSource(harnessId);
		endAttach(harnessId, token);
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
	getSessionId: () => string | undefined,
	onSessionFollowed: ((sessionId: string) => void) | undefined,
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
		case "session_created": {
			// #116: a child session (subagent, non-null parent_id) is
			// never a conversation to capture OR follow — Skein has no
			// business tracking it as the harness's own session id.
			if (event.parent_id !== null) return;
			const current = getSessionId();
			if (current === undefined) {
				// SSE-driven session-id capture (replaces chapter 5
				// phase 2b's sqlite poll on the happy path). The
				// callback short-circuits the sqlite fallback once
				// invoked.
				onSessionCaptured?.(event.session_id);
				return;
			}
			const followed = followedOpencodeSession(current, {
				kind: "session_created",
				sessionId: event.session_id,
				parentId: event.parent_id,
			});
			if (followed) {
				console.info(
					`[skein] opencode ${harnessId} followed onto ${followed.sessionId} (${followed.source})`,
				);
				onSessionFollowed?.(followed.sessionId);
			}
			return;
		}
		case "root_session_prompted": {
			// #116: the `/sessions` picker publishes nothing when it
			// switches onto an EXISTING root session — the first user
			// prompt landing there is the earliest honest signal. See
			// `followedOpencodeSession`.
			const current = getSessionId();
			if (current === undefined) {
				// No session captured yet: treat this as the initial
				// capture, not a follow — same reasoning as
				// `session_created`'s `current === undefined` arm.
				onSessionCaptured?.(event.session_id);
				return;
			}
			const followed = followedOpencodeSession(current, {
				kind: "root_session_prompted",
				sessionId: event.session_id,
			});
			if (followed) {
				console.info(
					`[skein] opencode ${harnessId} followed onto ${followed.sessionId} (${followed.source})`,
				);
				onSessionFollowed?.(followed.sessionId);
			}
			return;
		}
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
