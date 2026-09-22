// #116 — deciding whether to follow a `SessionStart` hook payload onto
// a new session id. Pure and separate from `harnessActivity.ts` so the
// decision is unit-testable without the store, same split
// `pendingPrompts.ts` and `subagents.ts` make for their own slices.
//
// `source === "clear"` and `source === "resume"` re-point the tail; the
// other sources the hook can report are all wrong to act on here:
//   - `startup` can be a phantom fire with a DIFFERENT session id from
//     the real one — upstream anthropics/claude-code#78455, see the
//     comment on `api_harness_session_start` in
//     `app/src-tauri/src/agent_api/http.rs`. Following it would chase
//     a conversation that never materialises.
//   - `compact` keeps the same session id — there is nothing to follow.
//   - `fork` and a missing/null source are left alone: no case for them
//     has been built yet. (Before Claude Code v2.1.214, a fork reported
//     `source: "resume"` with the fork's new id — following THAT is
//     still correct, since the conversation continues in the fork's own
//     file; only the dedicated `"fork"` source, reported from v2.1.214
//     on, is out of scope.)
//
// `resume` — a user typing `/resume` mid-session, not Skein's own
// resume-on-boot — reports the resumed conversation's ORIGINAL session
// id (Claude Code appends to the existing `<id>.jsonl`). Skein's own
// boot resume spawns `claude --resume <sid>`, which fires this same
// hook with the id Skein already has stored — the equality check below
// makes that a no-op, which is what makes following `resume` safe here.

/// Given the harness's current session id and a `harness-session-start`
/// payload, returns the session to follow, or `null` if this event
/// isn't one to act on. Returns `null` (not a result) when the reported
/// id equals `current` — nothing to re-point onto.
export function followedSession(
	current: string | undefined,
	payload: { sessionId?: string | null; source?: string | null },
): { sessionId: string; source: "clear" | "resume" } | null {
	const { source } = payload;
	if (source !== "clear" && source !== "resume") return null;
	const { sessionId } = payload;
	if (typeof sessionId !== "string" || sessionId.length === 0) return null;
	if (sessionId === current) return null;
	return { sessionId, source };
}
