// #568: argv for a `remote` harness — a tool run inside tmux on a
// remote host over ssh. Pure; built from the harness record only.

import type { RemoteSpec } from "./types.ts";

/** POSIX single-quote a string. */
export const shQuote = (s: string): string => `'${s.replaceAll("'", "'\\''")}'`;

/** A remote directory as a shell word. tmux never expands `~` and the
 *  quotes would block it anyway, so a leading `~/` (or a bare `~`) becomes
 *  a double-quoted "$HOME" that the remote login shell expands. */
const dirWord = (dir: string): string => {
	if (dir === "~") return '"$HOME"';
	if (dir.startsWith("~/")) return `"$HOME"/${shQuote(dir.slice(2))}`;
	return shQuote(dir);
};

/** Stable tmux session name. tmux forbids "." and ":" in names, so
 *  anything outside [A-Za-z0-9_-] becomes "_". */
export const remoteSessionName = (roomId: string, harnessId: string): string =>
	`skein-${roomId}-${harnessId}`.replace(/[^A-Za-z0-9_-]/g, "_");

/** An error message, or null when the host is usable. Rejects a leading
 *  "-" so a host can never be parsed by ssh as an option
 *  (`-oProxyCommand=...`). */
export const validateRemoteHost = (host: string): string | null => {
	const h = host.trim();
	if (h === "") return "Host is required.";
	// biome-ignore lint/suspicious/noControlCharactersInRegex: rejecting control chars is the point
	if (/[\s\u0000-\u001f\u007f]/.test(h)) return "Host must not contain whitespace.";
	if (h.startsWith("-")) return "Host must not start with '-'.";
	return null;
};

/** The ssh argv for a remote harness, or null when the host is invalid or
 *  the tool is empty.
 *
 *  The remote command is wrapped in `exec "$SHELL" -lc '...'`: a
 *  non-login ssh command sees no Homebrew / ~/.local PATH (measured on a
 *  macOS host: tmux and opencode not found without it). `new-session -A`
 *  attaches when the session already exists, so a Skein restart
 *  reattaches. The tool is ONE tmux shell-command argument, which tmux
 *  runs through the remote shell, so `~` and flags inside it work.
 *  `status off` hides tmux's status line so the TUI looks native and its
 *  clock doesn't keep resetting the L2a quiet timer. `--` stops ssh
 *  option parsing before the host. */
export const remoteArgv = (spec: RemoteSpec): string[] | null => {
	if (validateRemoteHost(spec.host) !== null) return null;
	if (spec.tool.trim() === "") return null;
	const { session, tool, dir } = spec;
	const inner =
		`tmux new-session -A -s ${shQuote(session)}` +
		(dir?.trim() ? ` -c ${dirWord(dir.trim())}` : "") +
		` ${shQuote(tool)} \\; set-option -t ${shQuote(session)} status off`;
	return ["ssh", "-t", "--", spec.host.trim(), `exec "$SHELL" -lc ${shQuote(inner)}`];
};

/** What the picker's remote step collects (before a harness id exists). */
export interface RemoteInput {
	host: string;
	tool: string;
	dir?: string;
}

/** The record's RemoteSpec: trimmed input plus the stable session name
 *  minted from the room and harness ids the record will carry. */
export const buildRemoteSpec = (
	roomId: string,
	harnessId: string,
	input: RemoteInput,
): RemoteSpec => {
	const dir = input.dir?.trim();
	return {
		host: input.host.trim(),
		tool: input.tool.trim(),
		session: remoteSessionName(roomId, harnessId),
		...(dir ? { dir } : {}),
	};
};
