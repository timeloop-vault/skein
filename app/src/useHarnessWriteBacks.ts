// The post-spawn write-backs that update one harness record after the
// fact (#19 split out of useHarnessCreation.ts): the captured/followed
// opencode or Claude session id, the post-exit-shell claim (#520) and
// the detected opencode major version (#675). Each is one `setRooms`
// updater over a pure transform in harnessCreation.ts.

import type { Dispatch, SetStateAction } from "react";
import {
	withCapturedSessionId,
	withOpencodeVersion,
	withoutShellClaim,
	withReplacedSessionId,
	withShellClaim,
} from "./harnessCreation.ts";
import type { OpencodeVersion, Room } from "./types.ts";

export function useHarnessWriteBacks(setRooms: Dispatch<SetStateAction<Room[]>>) {
	// Update one harness's sessionId after phase 2b's async capture
	// finds the new opencode row. Wrapped here so both creation paths
	// (pickHarness, createRoom) share the same setRooms shape.
	const setHarnessSessionId = (targetRoomId: string, harnessId: string, captured: string) => {
		setRooms((prev) => withCapturedSessionId(prev, targetRoomId, harnessId, captured));
	};

	// #116 step two: overwrite a harness's sessionId after Claude's own
	// `/clear`/`/resume`/fork hook reports the conversation moved onto a
	// new id — and (step four) after opencode's own adapter decides the
	// harness followed its TUI onto a different root session (`/new` or
	// a `/sessions` pick). This is deliberately NOT `setHarnessSessionId`
	// above — that one is first-writer-wins to survive opencode's
	// create-vs-poll capture race, but both these reports are
	// authoritative: the old id is definitely gone (Claude) or definitely
	// superseded (opencode), so the new one must win even though
	// `sessionId` is already set. `cmd` is untouched here — it's part of
	// LiveTerminal's mountKey, so changing it would respawn the PTY;
	// `resumeCmd` (harnessCmd.ts) rebuilds the argv from `sessionId` on
	// the next boot/reopen, which is what makes the new conversation the
	// one resumed.
	const replaceHarnessSessionId = (targetRoomId: string, harnessId: string, sessionId: string) => {
		setRooms((prev) => withReplacedSessionId(prev, targetRoomId, harnessId, sessionId));
	};

	// #520: persist the post-exit-shell claim (sessionId + claim in one
	// update) so a restart can resume it; release removes it again.
	const setHarnessShellClaim = (
		targetRoomId: string,
		harnessId: string,
		sessionId: string,
		port?: number,
	) => {
		setRooms((prev) => withShellClaim(prev, targetRoomId, harnessId, sessionId, port));
	};
	const setHarnessOpencodeVersion = (
		targetRoomId: string,
		harnessId: string,
		version: OpencodeVersion | null,
	) => {
		setRooms((prev) => withOpencodeVersion(prev, targetRoomId, harnessId, version));
	};
	const clearHarnessShellClaim = (targetRoomId: string, harnessId: string) => {
		setRooms((prev) => withoutShellClaim(prev, targetRoomId, harnessId));
	};

	return {
		setHarnessSessionId,
		replaceHarnessSessionId,
		setHarnessShellClaim,
		clearHarnessShellClaim,
		setHarnessOpencodeVersion,
	};
}
