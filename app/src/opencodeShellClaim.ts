// #517 — persisting an opencode process followed from the post-exit shell
// as the harness's `shellClaim` (#520), so a Skein restart resumes it
// instead of the shell. Only a PROVEN process (the caller has passed the
// startup proof) is ever claimed; while one is, every session binding —
// argv `--session` and SSE-followed `/new` alike — goes through the claim.
// Skein's own teardown never releases: opencode dies with Skein and the
// claim must survive for resume. Only the process going away does.
// Known limit: `pty_scan_opencode` picks the newest TUI opencode descendant,
// so a nested opencode TUI started under the shell takes over the claim
// until it exits.

import type { OpencodeScan } from "./opencodeShellFollow.ts";

/** Persists / removes the claim (`sessionId` stays on release, as claude's). */
export interface ShellClaimSink {
	set: (sessionId: string, port?: number) => void;
	release: () => void;
}

/** One transient empty scan must not lose the claim: release takes this many
 *  consecutive empty scans. */
export const NULL_SCANS_TO_RELEASE = 2;

export function createShellClaimTracker(d: {
	claim: ShellClaimSink | undefined;
	/** The plain session replace, used when there is no claim sink or no
	 *  proven process. */
	followed: ((sessionId: string) => void) | undefined;
}) {
	let pid: number | null = null;
	let session: string | null = null;
	let port: number | undefined;
	let nulls = 0;
	// What was last claimed, so the same pid reappearing after a release
	// re-claims it (never the argv id, which a later /new has outdated).
	let last: { pid: number; session: string; port: number | undefined } | null = null;

	const release = () => {
		if (pid !== null && session !== null) {
			d.claim?.release();
			last = { pid, session, port };
		}
		pid = null;
		session = null;
		port = undefined;
	};

	return {
		/** Any scan that found a process, proven or not: a different pid than
		 *  the claimed one means the claimed process is gone. */
		seen(scanPid: number) {
			nulls = 0;
			if (pid !== null && scanPid !== pid) release();
		},
		/** The scan found no process. */
		gone() {
			if (++nulls >= NULL_SCANS_TO_RELEASE) release();
		},
		/** A claim is held and a further empty scan would release it. */
		releasePending: () => pid !== null && session !== null,
		/** A proven scan; `newSession` is a session the argv re-bind adopts. */
		observe(scan: OpencodeScan, newSession: string | undefined) {
			if (!d.claim) {
				if (newSession !== undefined) d.followed?.(newSession);
				return;
			}
			if (pid === null && last !== null && last.pid === scan.pid && newSession === undefined) {
				pid = scan.pid;
				session = last.session;
				port = scan.portConfirmed && scan.port !== null ? scan.port : last.port;
				d.claim.set(session, port);
				return;
			}
			pid = scan.pid;
			const prevPort = port;
			const prevSession = session;
			if (scan.portConfirmed && scan.port !== null) port = scan.port;
			if (newSession !== undefined) session = newSession;
			else if (session === null) session = scan.sessionId;
			if (
				session !== null &&
				(newSession !== undefined || session !== prevSession || port !== prevPort)
			) {
				d.claim.set(session, port);
			}
		},
		/** A session change reported by the re-pointed SSE adapter. */
		followed(sessionId: string) {
			if (!d.claim || pid === null) {
				d.followed?.(sessionId);
				return;
			}
			session = sessionId;
			d.claim.set(sessionId, port);
		},
	};
}
