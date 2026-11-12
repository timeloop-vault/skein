// The lines a terminal mount writes into itself when the process is gone or
// never started (#731). Split out of useTerminalSpawn.ts (400-line cap).
// \x1b[2m = dim, \x1b[1m = bold, \x1b[31m = red, \x1b[0m = reset.

export const ENTER_FOR_SHELL = "\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell.\x1b[0m\r\n";

/** The exit line plus the shell prompt hint. */
export function exitedBanner(programName: string, code: number | null): string {
	const codeStr = code === null ? "?" : String(code);
	return `\r\n\x1b[2m[skein] ${programName} exited (${codeStr})\x1b[0m\r\n${ENTER_FOR_SHELL}`;
}

/** The start failure line; re-prompts so the user can tell Enter opens a shell. */
export function startFailedBanner(err: unknown): string {
	const msg = err instanceof Error ? err.message : String(err);
	return `\r\n\x1b[31m[skein] failed to start: ${msg}\x1b[0m\r\n${ENTER_FOR_SHELL}`;
}
