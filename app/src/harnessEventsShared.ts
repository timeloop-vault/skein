// Shared plumbing for the L2c adapters (epic #50): the throw guard every
// Channel handler is wrapped in. Used by `harnessEventsClaude.ts`,
// `harnessEventsOpencode.ts` and `harnessEventsHermes.ts`;
// `harnessEvents.ts` re-exports it.

import { logToRust } from "./frontendLog.ts";

/// How many `onmessage` throws per attach get logged before we stop
/// bothering Rust — a hot loop throwing on every message must not
/// flood the log file.
const MAX_GUARDED_ERRORS_LOGGED = 5;

/// Wrap a Channel's `onmessage` handler so a single throw can't wedge
/// the whole channel (#362). `@tauri-apps/api`'s `Channel` calls
/// `onmessage` BEFORE it advances `nextMessageIndex` (see
/// `@tauri-apps/api/core.js` around L95–97) — an uncaught throw from
/// one message leaves every LATER message parked in
/// `pendingMessages` forever, silently, even though Rust keeps
/// sending them successfully. Catching here and carrying on is what
/// keeps later messages arriving; logging is rate-limited to the
/// first `MAX_GUARDED_ERRORS_LOGGED` throws per attach. Exported so
/// the wrapping is testable without a real Tauri `Channel`.
export function guardChannelHandler<T extends { kind: string }>(
	harnessId: string,
	target: string,
	handler: (event: T) => void,
	log: (level: "error", target: string, message: string) => void = logToRust,
): (event: T) => void {
	let loggedErrors = 0;
	return (event: T) => {
		try {
			handler(event);
		} catch (err) {
			loggedErrors += 1;
			if (loggedErrors > MAX_GUARDED_ERRORS_LOGGED) return;
			const message = err instanceof Error ? err.message : String(err);
			const stack =
				err instanceof Error && typeof err.stack === "string" ? err.stack.slice(0, 1500) : "";
			log(
				"error",
				target,
				`onmessage threw harness=${harnessId} kind=${event.kind}: ${message}${
					stack ? `\nstack: ${stack}` : ""
				}`,
			);
		}
	};
}
