// L2c event-adapter attach for a freshly live PTY, split out of
// useTerminalSpawn.ts (#459).

import {
	attachClaudeEvents,
	attachOpencodeEvents,
	hasClaudeTranscriptTail,
} from "./harnessEvents.ts";
import type { HarnessKind } from "./types.ts";

export interface AdapterAttach {
	harnessId: string;
	roomId: string;
	cwd: string;
	harnessKind: HarnessKind;
	sessionId: string | undefined;
	opencodePort: number | undefined;
	onSessionCaptured: ((sessionId: string) => void) | undefined;
	onSessionFollowed: ((sessionId: string) => void) | undefined;
	sessionIdRef: { current: string | undefined };
	claudeAdapterRef: { current: { detach: () => void; sessionId: string } | null };
}

/** Attach the adapter that fits this harness; returns the opencode
 *  detach (the Claude one lives in `claudeAdapterRef` so the #116
 *  re-point effect can swap it), or null. */
export function attachAdapters(a: AdapterAttach): (() => void) | null {
	const { harnessId, roomId, cwd, harnessKind, sessionId, opencodePort } = a;
	// L2c-1 attach point: after PTY is alive, hook into the
	// Claude session log for authoritative running/waiting
	// signals. Only fires for Claude harnesses that own a
	// session uuid (chapter 5 `--session-id` pre-allocation).
	// The translator marks the activity store authoritative
	// once Rust confirms attach; until then L2a keeps
	// ticking, so a slow attach is a graceful degradation.
	if (hasClaudeTranscriptTail(harnessKind, a.sessionIdRef.current)) {
		const attachedSessionId = a.sessionIdRef.current;
		a.claudeAdapterRef.current = {
			detach: attachClaudeEvents(harnessId, roomId, attachedSessionId, cwd),
			sessionId: attachedSessionId,
		};
	}
	// L2c-2: attach the opencode SSE adapter when we have a
	// port (App allocated one via pick_free_port before the
	// spawn argv was finalized). Without a port the adapter
	// can't know where to subscribe — graceful fallback to
	// L2a + the sqlite-poll session-id capture.
	if (harnessKind === "opencode" && opencodePort !== undefined) {
		return attachOpencodeEvents(
			harnessId,
			roomId,
			cwd,
			opencodePort,
			sessionId,
			a.onSessionCaptured,
			// #116: live, not the `sessionId` closed over above —
			// this harness's session id can change under a
			// long-lived adapter (`/new`, a `/sessions` pick).
			() => a.sessionIdRef.current,
			a.onSessionFollowed,
		);
	}
	return null;
}
