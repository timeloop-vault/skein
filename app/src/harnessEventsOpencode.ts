// opencode half of the L2c adapter plumbing (epic #50, L2c-2): subscribes to
// the Rust `OpencodeEvent` channel (embedded-server SSE, fed by
// `harness_events_opencode.rs`). Since #732 the phase machine lives in the
// daemon, which tees this channel itself; what the webview still does with
// it is session capture/follow and the observed `--agent`. Public entry is
// `harnessEvents.ts`.

import { Channel, invoke } from "@tauri-apps/api/core";
import { observedAgents } from "./harnessAgent.ts";
import { guardChannelHandler } from "./harnessEventsShared.ts";
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
	// #175: assumed-idle baseline after every (re)connect.
	| { kind: "session_baseline_idle" }
	| { kind: "message_delta" }
	| { kind: "tool_use_start"; name: string }
	| { kind: "user_message_agent"; session_id: string; agent: string }
	// #86 — opencode's own permission and question prompts.
	| { kind: "permission_asked"; request_id: string; session_id: string }
	| { kind: "permission_replied"; request_id: string; session_id: string }
	| { kind: "question_asked"; request_id: string; session_id: string }
	| { kind: "question_resolved"; request_id: string; session_id: string }
	| { kind: "session_end" };

/// Subscribe an opencode harness to its embedded-server SSE stream on
/// `127.0.0.1:<port>`. `cwd` is the room worktree — the backend needs it
/// to capture a review baseline for each file the harness writes (#211).
/// Returns an unsubscribe.
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
/// follow.
///
/// Soft-fail: any Rust-side rejection (port closed, IPC dropped) warns;
/// the harness keeps running.
export function attachOpencodeEvents(
	harnessId: string,
	roomId: string,
	cwd: string,
	port: number,
	sessionId: string | undefined,
	onSessionCaptured: ((sessionId: string) => void) | undefined,
	getSessionId: () => string | undefined,
	onSessionFollowed: ((sessionId: string) => void) | undefined,
	/// #677: opencode v2 — the Skein-owned sidecar's generation. Rust then
	/// takes the sidecar's port + password itself and ignores `port`.
	sidecarGeneration?: number,
): () => void {
	const channel = new Channel<OpencodeEvent>();
	let closed = false;
	channel.onmessage = guardChannelHandler(harnessId, "opencode_events", (event) => {
		// A straggler from a closed attach must not translate.
		if (closed) return;
		translateOpencode(harnessId, event, onSessionCaptured, getSessionId, onSessionFollowed);
	});

	void invoke("opencode_events_attach", {
		harnessId,
		roomId,
		cwd,
		port,
		sessionId: sessionId ?? null,
		onEvent: channel,
		...(sidecarGeneration !== undefined ? { sidecarGeneration } : {}),
	}).catch((err: unknown) => {
		const msg = err instanceof Error ? err.message : String(err);
		console.warn(`[skein] opencode_events_attach failed for ${harnessId}:`, msg);
	});

	return () => {
		closed = true;
		// The process this was observed on is going away (#248).
		observedAgents.forget(harnessId);
		void invoke("opencode_events_detach", { harnessId }).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn(`[skein] opencode_events_detach failed for ${harnessId}:`, msg);
		});
	};
}

/// Exported for the tests. Only the non-phase effects remain (#732); every
/// other variant is the daemon's business.
export const translateOpencode = (
	harnessId: string,
	event: OpencodeEvent,
	onSessionCaptured: ((sessionId: string) => void) | undefined,
	getSessionId: () => string | undefined,
	onSessionFollowed: ((sessionId: string) => void) | undefined,
): void => {
	switch (event.kind) {
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
		case "user_message_agent":
			// Recorded with its session so the label can ignore a
			// subagent's child session (see `agentLabel`).
			observedAgents.record(harnessId, event.session_id, event.agent);
			return;
		default:
			return;
	}
};
