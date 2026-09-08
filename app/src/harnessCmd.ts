// Harness argv construction — fresh spawn and resume, in one place.
//
// Every property Skein has to decide *before* the child process starts
// lives here: Claude's pre-allocated `--session-id`, opencode's pinned
// `--port`. Both are spawn-time-locked, so the argv is the only place
// they can be expressed, and both boot and room-reopen have to be able
// to rebuild that argv from scratch.
//
// **Why this is a module and not two helpers in App.tsx.** `resumeCmd`
// used to identify a rebuildable argv by matching its *exact shape* —
// `cmd.length === 3 && cmd[1] === "--session-id"`. Every new spawn flag
// changes the length, so every new flag silently stopped matching, and
// an unmatched cmd was returned unchanged: the next boot respawned
// `--session-id <uuid>` against a session that already existed and
// Claude refused with "Session ID is already in use". That is #153, and
// it came back as #170, and it would have come back again the moment
// #219 appended `--agent`. Reconstructing from the harness *record*
// instead of from the previous argv retires the whole family — a new
// flag is now one line in `cmdForKind` plus one in `resumeCmd`, with
// nothing to keep in sync.

import { HARNESS_KINDS } from "./data.tsx";
import type { Harness, HarnessKind, Room } from "./types.ts";

/** The program Skein spawns for a kind, or null where Skein doesn't
 *  choose it (`byoh` takes the user's shell; `files` has no process).
 *  Used as the ownership test in `resumeCmd` — see there. */
const managedProgram = (kind: HarnessKind): string | null => {
	switch (kind) {
		case "claude":
			return "claude";
		case "opencode":
			return "opencode";
		case "copilot":
			return "gh";
		case "byoh":
		case "files":
			return null;
	}
};

// Phase 2a: when sessionId is provided (always set by callers for
// Claude, never for other kinds), pre-allocate Claude's conversation
// id via --session-id <uuid>. Storing the same id on the harness
// record lets phase 3 resume directly with no picker.
//
// Epic #50 L2c-2: opencode embeds an HTTP server. Skein allocates a
// free port up front and passes `--port <N> --hostname 127.0.0.1` so
// the L2c-2 SSE adapter knows where to subscribe. The port is fresh
// per spawn (not persisted) — callers must pass one in for opencode.
export const cmdForKind = (
	kind: HarnessKind,
	fallbackShell: string[],
	sessionId?: string,
	opencodePort?: number,
): string[] => {
	switch (kind) {
		case "claude":
			return sessionId ? ["claude", "--session-id", sessionId] : ["claude"];
		case "opencode": {
			// Default port=0 lets opencode pick; that defeats the whole
			// adapter, so require an allocated port. If a caller forgot,
			// fall back to bare opencode and the adapter just won't
			// attach — same behaviour as pre-L2c-2.
			if (opencodePort === undefined) return ["opencode"];
			return ["opencode", "--port", String(opencodePort), "--hostname", "127.0.0.1"];
		}
		case "copilot":
			return ["gh", "copilot", "suggest"];
		case "byoh":
			return fallbackShell.length > 0 ? fallbackShell : ["pwsh.exe"];
		case "files":
			// Unreachable: `files` has no process (capabilities.pty is
			// false, so creation paths never call cmdForKind for it).
			// The empty argv is a type-totality placeholder, and
			// HarnessBody wouldn't spawn an empty cmd anyway.
			return [];
	}
};

/** Rebuild a harness's argv into its "resume the previous
 *  conversation" form. Applied at boot and on room reopen, so a fresh
 *  PTY spawn transparently re-attaches instead of starting over.
 *
 *  Derived entirely from the harness record — `kind`, `sessionId`, plus
 *  the freshly allocated `opencodePort` — never from the argv it
 *  replaces. Idempotent: feeding a resume-form cmd back in yields the
 *  same cmd (with the new port for opencode), which is what makes it
 *  safe to run on every boot and every reopen.
 *
 *  Three sources of session ids feed in:
 *    1. harness.sessionId set by phase 2a (Claude pre-allocate).
 *    2. harness.sessionId set by phase 2b (opencode capture-after-spawn).
 *    3. None — legacy harness created before chapter 5, or capture
 *       timed out. We fall back to chapter 2 phase 5a's behaviour:
 *       Claude shows its picker, opencode resumes most-recent-in-cwd. */
export const resumeCmd = (h: Harness, opencodePort?: number): string[] => {
	const cmd = h.cmd ?? [];
	// Capability gate first (#184): kinds without a resume concept
	// (copilot, shell, files) pass through untouched.
	if (!HARNESS_KINDS[h.kind].capabilities.resume) return cmd;
	// Only rebuild argvs Skein still owns. The app has exactly one
	// cmd-mutation path — Enter-for-shell after a child exits
	// (LiveTerminal.tsx), which replaces the whole argv with the user's
	// shell — so argv[0] is a sufficient ownership test, and unlike the
	// old argv-length matching it does not care how many flags Skein
	// adds. A shell-swapped harness keeps its shell; rebuilding it into
	// `claude --resume` would resurrect a harness the user retired.
	if (cmd[0] !== managedProgram(h.kind)) return cmd;
	switch (h.kind) {
		case "claude":
			return h.sessionId ? ["claude", "--resume", h.sessionId] : ["claude", "--resume"];
		case "opencode": {
			// The port from sqlite is dead — the previous Skein run
			// released it when the harness exited — so a fresh one is
			// baked in on every rebuild.
			const args = ["opencode"];
			if (opencodePort !== undefined) {
				args.push("--port", String(opencodePort), "--hostname", "127.0.0.1");
			}
			if (h.sessionId) args.push("--session", h.sessionId);
			else args.push("--continue");
			return args;
		}
		default:
			return cmd;
	}
};

/** Every harness in the room rewritten to resume form. Harnesses with
 *  no cmd (the `files` kind, or a record that never spawned) pass
 *  through untouched — `cmd: undefined` must stay absent rather than
 *  become an empty argv, or HarnessBody would try to spawn it. */
export const withResumeCmds = (room: Room, ports: ReadonlyMap<string, number>): Room => ({
	...room,
	harnesses: room.harnesses.map((h) => (h.cmd ? { ...h, cmd: resumeCmd(h, ports.get(h.id)) } : h)),
});

/** Un-archive a room: drop the `archived` stamp and put every harness
 *  back into resume form. Pure, so the three callers that un-archive
 *  (the reopen modal, the palette, an OS-notification click — #170) can
 *  share one transform instead of each remembering to do both halves. */
export const unarchiveRoomTransform = (room: Room, ports: ReadonlyMap<string, number>): Room => {
	const { archived, ...rest } = room;
	return withResumeCmds(rest, ports);
};
