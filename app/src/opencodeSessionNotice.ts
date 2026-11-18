// #679: the daemon did not resume a stored opencode session because it
// belongs to the other opencode major version. The spawn result says so
// (`sessionNotice`); this is the wording and the in-pane hint it shows.
// The v2 TUI is full screen, so it goes in the hint overlay, not into xterm.

import type { EnsureResult, SessionNotice } from "./harnessEnsure.ts";
import type { OpencodeVersion } from "./types.ts";

/** Long enough to read three sentences; the default hints are 1-8 s. */
export const SESSION_NOTICE_HINT_MS = 25000;

export function sessionNoticeHint(
	notice: SessionNotice,
	version: OpencodeVersion | null | undefined,
): string {
	const id = notice.sessionId;
	if (notice.kind === "v1SessionUnderV2") {
		const v = version?.version ?? "2";
		return `Session ${id} was created by opencode 1 after opencode ${v} migrated its sessions, so opencode ${v} cannot see it. Skein started a fresh session instead of an empty one under the old id. The old conversation is still in opencode 1's history.`;
	}
	const v = version?.version ?? "1";
	return `Session ${id} is an opencode 2 session and opencode ${v} cannot open it. Skein started a fresh session.`;
}

export interface NoticeHintSink {
	setHint: (hint: string | null) => void;
	hintTimerRef: { current: ReturnType<typeof setTimeout> | null };
}

/** Shows the notice for a result that started a process; a re-attach to a
 *  running one reports nothing. Returns whether a hint was shown. */
export function showSessionNotice(result: EnsureResult, sink: NoticeHintSink): boolean {
	const notice = result.sessionNotice;
	if (!result.started || !notice) return false;
	const text = sessionNoticeHint(notice, result.opencodeVersion);
	console.warn(`[skein] ${text}`);
	if (sink.hintTimerRef.current) clearTimeout(sink.hintTimerRef.current);
	sink.setHint(text);
	sink.hintTimerRef.current = setTimeout(() => {
		sink.setHint(null);
		sink.hintTimerRef.current = null;
	}, SESSION_NOTICE_HINT_MS);
	return true;
}
