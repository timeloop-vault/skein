// #116 — deciding whether to follow a `SessionStart` hook payload onto
// a new session id. Pure and separate from `harnessActivity.ts` so the
// decision is unit-testable without the store, same split
// `pendingPrompts.ts` and `subagents.ts` make for their own slices.
//
// Only `source === "clear"` re-points the tail. The other sources the
// hook can report are all wrong to act on here:
//   - `startup` can be a phantom fire with a DIFFERENT session id from
//     the real one — upstream anthropics/claude-code#78455, see the
//     comment on `api_harness_session_start` in
//     `app/src-tauri/src/agent_api/http.rs`. Following it would chase
//     a conversation that never materialises.
//   - `compact` keeps the same session id — there is nothing to follow.
//   - `resume` (a user typing `/resume` mid-session, not Skein's own
//     resume-on-boot) is deliberately out of scope for this step.
//   - `fork` and a missing/null source are left alone for the same
//     reason: no case for them has been built yet.

/// Given the harness's current session id and a `harness-session-start`
/// payload, returns the new session id to follow, or `null` if this
/// event isn't one to act on. Returns `null` (not the new id) when the
/// reported id equals `current` — nothing to re-point onto.
export function clearedSessionId(
	current: string | undefined,
	payload: { sessionId?: string | null; source?: string | null },
): string | null {
	if (payload.source !== "clear") return null;
	const { sessionId } = payload;
	if (typeof sessionId !== "string" || sessionId.length === 0) return null;
	if (sessionId === current) return null;
	return sessionId;
}
