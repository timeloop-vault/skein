// #116 — deciding whether to follow a `SessionStart` hook payload onto
// a new session id. Pure and separate from `harnessActivity.ts` so the
// decision is unit-testable without the store, same split
// `pendingPrompts.ts` and `subagents.ts` make for their own slices.
//
// `source === "clear"`, `"resume"` and `"fork"` all re-point the tail;
// the other sources the hook can report are all wrong to act on here:
//   - `startup` can be a phantom fire with a DIFFERENT session id from
//     the real one — upstream anthropics/claude-code#78455, see the
//     comment on `api_harness_session_start` in
//     `app/src-tauri/src/agent_api/http.rs`. Following it would chase
//     a conversation that never materialises.
//   - `compact` keeps the same session id — there is nothing to follow.
//   - a missing/null source is left alone: no case for it has been
//     built yet.
//
// `resume` — a user typing `/resume` mid-session, not Skein's own
// resume-on-boot — reports the resumed conversation's ORIGINAL session
// id (Claude Code appends to the existing `<id>.jsonl`). Skein's own
// boot resume spawns `claude --resume <sid>`, which fires this same
// hook with the id Skein already has stored — the equality check below
// makes that a no-op, which is what makes following `resume` safe here.
//
// `fork` — verified upstream: triggered by `/branch`, `/fork`,
// `--fork-session` or a desktop rewind, it carries the NEWLY forked
// session id, which gets its own new transcript file from the fork
// point on. Following it is correct for the same reason `clear` and
// `resume` are — the live conversation has moved to a different id and
// nothing else will ever report that. (Before Claude Code v2.1.214, a
// fork reported `source: "resume"` with the fork's new id instead —
// that shape was already covered by the `resume` arm above.)

/// Given the harness's current session id and a `harness-session-start`
/// payload, returns the session to follow, or `null` if this event
/// isn't one to act on. Returns `null` (not a result) when the reported
/// id equals `current` — nothing to re-point onto.
export function followedSession(
	current: string | undefined,
	payload: { sessionId?: string | null; source?: string | null },
): { sessionId: string; source: "clear" | "resume" | "fork" } | null {
	const { source } = payload;
	if (source !== "clear" && source !== "resume" && source !== "fork") return null;
	const { sessionId } = payload;
	if (typeof sessionId !== "string" || sessionId.length === 0) return null;
	if (sessionId === current) return null;
	return { sessionId, source };
}

// ── opencode ─────────────────────────────────────────────────────────
//
// opencode has no `SessionStart`-style hook to tell Skein when the TUI
// moves onto a different root session — `/new` and the `/sessions`
// picker both change what the user is looking at with no event
// published upstream (sst/opencode#5409 tracks this). So Skein infers
// the move from the two SSE signals it does get:
//
//   - `session_created` for a ROOT session (no `parentId`) fires the
//     instant `/new` (or the picker's "new session" entry) creates one
//     — the earliest honest signal for that case. A session with a
//     `parentId` is a subagent child, never a conversation to follow.
//   - the `/sessions` picker publishes nothing at all when it switches
//     onto an EXISTING root session, so the first user prompt landing
//     in that other session (`root_session_prompted`, backend-verified
//     non-child) is the earliest honest signal for that case.
//
// Both arms return `null` when `current` is `undefined` — that is the
// harness's very first session, which the existing first-writer-wins
// `onSessionCaptured` path already owns; treating it as a "follow"
// here would just race the same write through a second door.

export type OpencodeSessionFollow = { sessionId: string; source: "new" | "switch" };

export function followedOpencodeSession(
	current: string | undefined,
	event:
		| { kind: "session_created"; sessionId: string; parentId: string | null }
		| { kind: "root_session_prompted"; sessionId: string },
): OpencodeSessionFollow | null {
	if (event.kind === "session_created") {
		if (event.parentId !== null) return null;
		if (current === undefined) return null;
		if (event.sessionId === current) return null;
		return { sessionId: event.sessionId, source: "new" };
	}
	if (current === undefined) return null;
	if (event.sessionId === current) return null;
	return { sessionId: event.sessionId, source: "switch" };
}
