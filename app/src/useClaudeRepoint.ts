// #116: re-point the Claude JSONL adapter, split out of useTerminalSpawn.ts.

import { useEffect } from "react";
import { attachClaudeEvents } from "./harnessEvents.ts";
import { shellClaim } from "./shellClaim.ts";

export function useClaudeRepoint(p: {
	sessionId: string | undefined;
	harnessId: string;
	roomId: string;
	cwd: string;
	claudeAdapterRef: { current: { detach: () => void; sessionId: string } | null };
}): void {
	const { sessionId, harnessId, roomId, cwd, claudeAdapterRef } = p;
	// #116: re-point the Claude JSONL adapter when `sessionId` changes
	// under an already-running PTY — Claude's own `/clear` or in-tool
	// `/resume` moving to a different conversation, reported via
	// App.tsx's `skein://harness-session-start` listener updating the
	// harness record, which flows back down here as a new prop. The PTY
	// itself is untouched: only the tail target moves. A no-op when no
	// adapter is attached (PTY not live yet, or a non-Claude harness) —
	// the mount effect's own attach (above) picks up the current
	// sessionId whenever it eventually runs.
	useEffect(() => {
		const current = claudeAdapterRef.current;
		if (!current) return;
		if (typeof sessionId !== "string" || sessionId === current.sessionId) return;
		current.detach();
		const fresh = shellClaim.consumeFreshProcess(harnessId); // #318
		claudeAdapterRef.current = {
			// #336: the process has lived across this re-point; a long tool
			// call may be genuinely live, so no fresh-process replay —
			// except #318's shell-claimed `claude`, a new process.
			detach: attachClaudeEvents(harnessId, roomId, sessionId, cwd, fresh),
			sessionId,
		};
	}, [sessionId, harnessId, roomId, cwd, claudeAdapterRef]);
}
