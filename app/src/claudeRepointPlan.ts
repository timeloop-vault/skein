// What the Claude transcript adapter should do now (#763, #116): the pure
// decision behind useClaudeRepoint.

export interface ClaudeRepointInput {
	isClaude: boolean;
	/** The harness runs its tool (not an unclaimed post-exit shell). */
	runsTool: boolean;
	/** The PTY is running now (ensured live, not exited, still mounted). An
	 *  exited pane has no live transcript, so it never attaches. */
	ptyLive: boolean;
	sessionId: string | undefined;
	/** In shell launch mode: the claimed session. Attaching waits for
	 *  `sessionId` to catch up to it, so the old session is never replayed. */
	claimSessionId: string | undefined;
	/** The session the attached adapter tails; null = none attached. */
	attachedSessionId: string | null;
}

export type ClaudeRepointAction = "none" | "attach" | "repoint" | "detach";

/** A shell claim's session has reached the record (or there is no claim). */
export const claimSettled = (claim: string | undefined, sessionId: string | undefined): boolean =>
	claim === undefined || claim === sessionId;

export function planClaudeRepoint(i: ClaudeRepointInput): ClaudeRepointAction {
	if (i.attachedSessionId !== null && !i.runsTool) return "detach";
	if (!i.isClaude || !i.runsTool || !i.sessionId) return "none";
	if (i.attachedSessionId === null) {
		if (!i.ptyLive) return "none";
		if (!claimSettled(i.claimSessionId, i.sessionId)) return "none";
		return "attach";
	}
	return i.attachedSessionId === i.sessionId ? "none" : "repoint";
}
