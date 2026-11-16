// #116: re-point the Claude JSONL adapter, split out of useTerminalSpawn.ts.
// #763: also attach it when a post-exit shell gains a claim, and detach it
// when the harness stops running its tool.

import { useEffect } from "react";
import { planClaudeRepoint } from "./claudeRepointPlan.ts";
import { attachClaudeEvents } from "./harnessEvents.ts";
import { shellClaim } from "./shellClaim.ts";
import type { HarnessKind } from "./types.ts";

export function useClaudeRepoint(p: {
	sessionId: string | undefined;
	harnessKind: HarnessKind;
	runsTool: boolean;
	claimSessionId: string | undefined;
	liveRef: { current: boolean };
	harnessId: string;
	roomId: string;
	cwd: string;
	claudeAdapterRef: { current: { detach: () => void; sessionId: string } | null };
}): void {
	const {
		sessionId,
		harnessKind,
		runsTool,
		claimSessionId,
		liveRef,
		harnessId,
		roomId,
		cwd,
		claudeAdapterRef,
	} = p;
	// #116: re-point the Claude JSONL adapter when `sessionId` changes
	// under an already-running PTY — Claude's own `/clear` or in-tool
	// `/resume` moving to a different conversation, reported via
	// App.tsx's `skein://harness-session-start` listener updating the
	// harness record, which flows back down here as a new prop. The PTY
	// itself is untouched: only the tail target moves. With no adapter
	// attached the mount effect's own attach picks up the current
	// sessionId when it runs — except #763's skipped attach (post-exit
	// shell): once the PTY is live and a claim makes the harness run its
	// tool again, this effect attaches, in either order of the claim and
	// the session id landing (it waits for the claimed id). The mount
	// effect's own attach is guarded by `liveRef`, false
	// until a live ensure resolves and again once the pane exits.
	useEffect(() => {
		const current = claudeAdapterRef.current;
		const action = planClaudeRepoint({
			isClaude: harnessKind === "claude",
			runsTool,
			ptyLive: liveRef.current,
			sessionId,
			claimSessionId,
			attachedSessionId: current?.sessionId ?? null,
		});
		if (action === "detach") {
			current?.detach();
			claudeAdapterRef.current = null;
		} else if (action === "attach" || action === "repoint") {
			if (typeof sessionId !== "string") return;
			current?.detach();
			const fresh = shellClaim.consumeFreshProcess(harnessId); // #318
			claudeAdapterRef.current = {
				// #336: the process has lived across this re-point; a long tool
				// call may be genuinely live, so no fresh-process replay —
				// except #318's shell-claimed `claude`, a new process.
				detach: attachClaudeEvents(harnessId, roomId, sessionId, cwd, fresh),
				sessionId,
			};
		}
	}, [
		sessionId,
		harnessKind,
		runsTool,
		claimSessionId,
		liveRef,
		harnessId,
		roomId,
		cwd,
		claudeAdapterRef,
	]);
}
