// The post-spawn write-backs that update one harness record after the
// fact (#19 split out of useHarnessCreation.ts): the captured/followed
// opencode or Claude session id and the post-exit-shell claim (#520).
//
// #755: `Harness.sessionId` and `Harness.opencodeVersion` are daemon-owned.
// The session id is therefore never written into rooms state here: the
// client REPORTS what it observed (`reportHarnessSession`) and the changed
// record comes back on skein://rooms-changed one round-trip later. A failed
// report is logged and not retried. The opencode version needs no write-back
// at all: the runner records it at every spawn, before ensure returns.
// `shellClaim` is still client-written (one `setRooms` over a pure transform
// in harnessCreation.ts).

import type { Dispatch, SetStateAction } from "react";
import { withoutShellClaim, withShellClaim } from "./harnessCreation.ts";
import { reportHarnessSession, type SessionReport } from "./harnessSessionReport.ts";
import { noteReported, settleReported } from "./reportedSessions.ts";
import type { Room } from "./types.ts";

const report = (roomId: string, harnessId: string, what: SessionReport) => {
	// #755: remember the id until the record shows it (reportedSessions.ts).
	const id = what.mode === "clear" ? null : what.sessionId;
	if (id !== null) noteReported(harnessId, id);
	reportHarnessSession(roomId, harnessId, what).then(
		(changed) => {
			if (id !== null) settleReported(harnessId, id, changed);
		},
		(err: unknown) => {
			if (id !== null) settleReported(harnessId, id, false);
			console.error(`[skein] harness_session_report (${what.mode}) failed for ${harnessId}`, err);
		},
	);
};

export function useHarnessWriteBacks(setRooms: Dispatch<SetStateAction<Room[]>>) {
	// Report a harness's session id after phase 2b's async capture finds the
	// new opencode row. First writer wins, and the daemon decides which one
	// that was, so both creation paths and the SSE adapter can all report.
	const setHarnessSessionId = (targetRoomId: string, harnessId: string, captured: string) => {
		report(targetRoomId, harnessId, { mode: "capture", sessionId: captured });
	};

	// #116 step two: report that the conversation moved onto a new id after
	// Claude's own `/clear`/`/resume`/fork hook — and (step four) after
	// opencode's own adapter decides the harness followed its TUI onto a
	// different root session (`/new` or a `/sessions` pick). This is
	// deliberately NOT `setHarnessSessionId` above — that one is
	// first-writer-wins to survive opencode's create-vs-poll capture race,
	// but both these reports are authoritative: the old id is definitely
	// gone (Claude) or definitely superseded (opencode), so the new one must
	// win even though `sessionId` is already set. `cmd` is untouched here —
	// it's part of LiveTerminal's mountKey, so changing it would respawn the
	// PTY; the daemon (harness_argv/) rebuilds the argv from `sessionId` on
	// the next boot/reopen, which is what makes the new conversation the
	// one resumed.
	const replaceHarnessSessionId = (targetRoomId: string, harnessId: string, sessionId: string) => {
		report(targetRoomId, harnessId, { mode: "replace", sessionId });
	};

	// #520: persist the post-exit-shell claim so a restart can resume it;
	// release removes it again. The claim is client-written; the session id
	// it names is daemon-owned (#755), so it is reported (replace) alongside.
	const setHarnessShellClaim = (
		targetRoomId: string,
		harnessId: string,
		sessionId: string,
		port?: number,
	) => {
		report(targetRoomId, harnessId, { mode: "replace", sessionId });
		setRooms((prev) => withShellClaim(prev, targetRoomId, harnessId, sessionId, port));
	};
	const clearHarnessShellClaim = (targetRoomId: string, harnessId: string) => {
		setRooms((prev) => withoutShellClaim(prev, targetRoomId, harnessId));
	};

	return {
		setHarnessSessionId,
		replaceHarnessSessionId,
		setHarnessShellClaim,
		clearHarnessShellClaim,
	};
}
