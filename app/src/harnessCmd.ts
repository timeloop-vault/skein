// Harness argv construction — fresh spawn and resume, in one place.
//
// Every property Skein has to decide *before* the child process starts
// lives here: Claude's pre-allocated `--session-id`, opencode's pinned
// `--port`, and the `--agent` both CLIs bind at launch (#247). All
// three are spawn-time-locked, so the argv is the only place they can
// be expressed, and both boot and room-reopen have to be able to
// rebuild that argv from scratch.
//
// **Why this is a module and not two helpers in App.tsx.** `resumeCmd`
// used to identify a rebuildable argv by matching its *exact shape* —
// `cmd.length === 3 && cmd[1] === "--session-id"`. Every new spawn flag
// changes the length, so every new flag silently stopped matching, and
// an unmatched cmd was returned unchanged: the next boot respawned
// `--session-id <uuid>` against a session that already existed and
// Claude refused with "Session ID is already in use". That is #153, and
// it came back as #170, and it would have come back a third time when
// #247 appended `--agent` — which it did not, because reconstructing
// from the harness *record* instead of from the previous argv retired
// the whole family. `--agent` cost one `withAgent` call in each arm,
// with nothing to keep in sync.

import { HARNESS_KINDS } from "./data.tsx";
import { remoteArgv } from "./remoteCmd.ts";
import type { Harness, HarnessKind, RemoteSpec, Room } from "./types.ts";

/** Append `--agent <name>` when the harness names one.
 *
 *  Both CLIs spell it the same way, so this is shared. An empty or
 *  whitespace-only name is treated as absent rather than passed
 *  through: `--agent ""` is refused by Claude at spawn, and the only
 *  way to get one is a stored blob that has been hand-edited. */
const withAgent = (args: string[], agent: string | undefined): string[] =>
	agent?.trim() ? [...args, "--agent", agent] : args;

// Phase 2a: when sessionId is provided (always set by callers for
// Claude, never for other kinds), pre-allocate Claude's conversation
// id via --session-id <uuid>. Storing the same id on the harness
// record lets phase 3 resume directly with no picker.
//
// Epic #50 L2c-2: opencode embeds an HTTP server. Skein allocates a
// free port up front and passes `--port <N> --hostname 127.0.0.1` so
// the L2c-2 SSE adapter knows where to subscribe. The port is fresh
// per spawn (not persisted) — callers must pass one in for opencode.
// Issue #247: `agent` is the third spawn-time-locked decision. Claude
// binds `--agent` at launch and cannot change it afterwards, so like
// the session id and the port it can only be expressed in the argv.
// Undefined means "no flag" — the tool's own `agent` setting decides,
// which is a real choice and not the absence of one.
export const cmdForKind = (
	kind: HarnessKind,
	fallbackShell: string[],
	sessionId?: string,
	opencodePort?: number,
	agent?: string,
	remote?: RemoteSpec,
): string[] => {
	switch (kind) {
		case "claude": {
			const args = sessionId ? ["claude", "--session-id", sessionId] : ["claude"];
			return withAgent(args, agent);
		}
		case "opencode": {
			// Default port=0 lets opencode pick; that defeats the whole
			// adapter, so require an allocated port. If a caller forgot,
			// fall back to bare opencode and the adapter just won't
			// attach — same behaviour as pre-L2c-2.
			if (opencodePort === undefined) return withAgent(["opencode"], agent);
			return withAgent(
				["opencode", "--port", String(opencodePort), "--hostname", "127.0.0.1"],
				agent,
			);
		}
		case "copilot":
			return ["gh", "copilot", "suggest"];
		case "byoh":
			return fallbackShell.length > 0 ? fallbackShell : ["pwsh.exe"];
		case "remote":
			return (remote && remoteArgv(remote)) ?? [];
		case "files":
		case "design":
			// Unreachable: `files`/`design` have no process (capabilities.pty is
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
 *  A shell argv is passed through, except that a LIVE `shellClaim`
 *  (#520, see `resumeShell`) turns it back into `claude --resume`.
 *
 *  Three sources of session ids feed in:
 *    1. harness.sessionId set by phase 2a (Claude pre-allocate).
 *    2. harness.sessionId set by phase 2b (opencode capture-after-spawn).
 *    3. None — legacy harness created before chapter 5, or capture
 *       timed out. Claude starts a FRESH session (`--session-id <new
 *       uuid>`, #486) rather than bare `--resume` (which opens its
 *       picker) or `--continue` (which could latch onto another
 *       harness's conversation in the same worktree). opencode keeps
 *       resuming most-recent-in-cwd (`--continue`).
 *
 *  Case 3 for Claude mints an id this function CANNOT persist: it
 *  returns only an argv. Any caller that needs telemetry or the next
 *  resume to work must use `resumeHarness`, which writes the id back. */
export const resumeCmd = (h: Harness, opencodePort?: number): string[] =>
	resumeHarness(h, opencodePort).cmd ?? [];

const hasSessionId = (h: Harness): boolean => (h.sessionId ?? "").trim() !== "";

const withoutClaim = (h: Harness): Harness => {
	const { shellClaim: _consumed, ...rest } = h;
	return rest;
};

/** #520: a harness whose stored argv is the user's shell (Enter after
 *  the child exited). Derived from the record, never the argv: a
 *  `shellClaim` is LIVE when its `sessionId` is non-empty and equals
 *  `h.sessionId`. Hydrate/unarchive's existence probe drops a
 *  `sessionId` whose transcript is gone, so a claim that no longer
 *  matches was a phantom or was dropped. A live claim puts the harness
 *  back on its own program (`claude --resume <sid>`, and the claim is
 *  consumed — the field is removed); a stale one restores the shell
 *  (claim removed, cmd unchanged). Claude only; other kinds keep their
 *  `shellClaim` untouched. opencode (#517) resumes the same way via
 *  `opencodeResumeArgv`. */
const resumeShell = (h: Harness, cmd: string[], opencodePort?: number): Harness => {
	if ((h.kind !== "claude" && h.kind !== "opencode") || !h.shellClaim) return { ...h, cmd };
	const sid = h.shellClaim.sessionId;
	if (sid.trim() !== "" && sid === h.sessionId) {
		// The persisted `claim.port` is deliberately NOT reused: the previous
		// run's port is dead and may be taken now, and opencode fails hard on
		// a busy explicit --port rather than falling back.
		const argv =
			h.kind === "claude" ? ["claude", "--resume", sid] : opencodeResumeArgv(sid, opencodePort);
		return { ...withoutClaim(h), cmd: withAgent(argv, h.agent) };
	}
	return { ...withoutClaim(h), cmd };
};

/** The opencode resume argv (before `--agent`), shared by the rebuild
 *  and the shell-claim paths so they cannot drift. */
const opencodeResumeArgv = (sessionId: string | undefined, port?: number): string[] => {
	const args = ["opencode"];
	if (port !== undefined) args.push("--port", String(port), "--hostname", "127.0.0.1");
	if (sessionId) args.push("--session", sessionId);
	else args.push("--continue");
	return args;
};

/** `resumeCmd`, but returning the whole harness so a session id minted
 *  here (Claude with none stored, #486) lands on the record. */
export const resumeHarness = (
	h: Harness,
	opencodePort?: number,
	mintSessionId: () => string = () => crypto.randomUUID(),
): Harness => {
	const cmd = h.cmd ?? [];
	// Capability gate first (#184): kinds without a resume concept
	// (copilot, shell, files) pass through untouched.
	if (!HARNESS_KINDS[h.kind].capabilities.resume) return { ...h, cmd };
	// Only rebuild argvs Skein still owns. The app has exactly one
	// cmd-mutation path — Enter-for-shell after a child exits
	// (LiveTerminal.tsx), which replaces the whole argv with the user's
	// shell — so argv[0] is a sufficient ownership test, and unlike the
	// old argv-length matching it does not care how many flags Skein
	// adds. A shell-swapped harness keeps its shell; rebuilding it into
	// `claude --resume` would resurrect a harness the user retired.
	if (cmd[0] !== HARNESS_KINDS[h.kind].program) return resumeShell(h, cmd, opencodePort);
	if (h.kind === "claude" && h.shellClaim) {
		return resumeHarness(withoutClaim(h), opencodePort, mintSessionId);
	}
	switch (h.kind) {
		case "claude": {
			if (!hasSessionId(h)) {
				const id = mintSessionId();
				return { ...h, sessionId: id, cmd: withAgent(["claude", "--session-id", id], h.agent) };
			}
			// #247: the agent is re-passed, not left to the session.
			// `claude --resume <sid>` with no flag restores whatever agent
			// the conversation *started* as — so dropping it here would
			// make the record and the process disagree the first time the
			// user changes a harness's agent, silently and permanently.
			// Re-passing overrides cleanly (measured on #219).
			return { ...h, cmd: withAgent(["claude", "--resume", h.sessionId as string], h.agent) };
		}
		case "opencode": {
			// The port from sqlite is dead — the previous Skein run
			// released it when the harness exited — so a fresh one is
			// baked in on every rebuild.
			return { ...h, cmd: withAgent(opencodeResumeArgv(h.sessionId, opencodePort), h.agent) };
		}
		case "remote": {
			// #568: tmux `-A` reattaches, so the same argv is right every time.
			const argv = h.remote ? remoteArgv(h.remote) : null;
			return { ...h, cmd: argv ?? cmd };
		}
		default:
			return { ...h, cmd };
	}
};

/** Every harness in the room rewritten to resume form. Harnesses with
 *  no cmd (the `files` kind, or a record that never spawned) pass
 *  through untouched — `cmd: undefined` must stay absent rather than
 *  become an empty argv, or HarnessBody would try to spawn it. */
export const withResumeCmds = (room: Room, ports: ReadonlyMap<string, number>): Room => ({
	...room,
	harnesses: room.harnesses.map((h) => (h.cmd ? resumeHarness(h, ports.get(h.id)) : h)),
});

/** Un-archive a room: drop the `archived` stamp and put every harness
 *  back into resume form. Pure, so the three callers that un-archive
 *  (the reopen modal, the palette, an OS-notification click — #170) can
 *  share one transform instead of each remembering to do both halves.
 *  #411: also drops `closedBy` — a reopened room was never "closed by"
 *  anyone any more, and letting it survive would misattribute whatever
 *  happens next. */
export const unarchiveRoomTransform = (room: Room, ports: ReadonlyMap<string, number>): Room => {
	const { archived, retired: _retired, closedBy: _closedBy, ...rest } = room;
	return withResumeCmds(rest, ports);
};
