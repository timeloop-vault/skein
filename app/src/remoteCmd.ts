// #568: a `remote` harness - a tool run inside tmux on a remote host over
// ssh. The argv is built by the daemon (#730, harness_argv/remote.rs); this
// keeps the UI validation and the record building. Pure.

import type { RemoteSpec } from "./types.ts";

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
