// Fire-and-forget forwarding of frontend log lines to Rust's
// daily-rotating `skein.log`, via the `frontend_log` Tauri command
// (`app/src-tauri/src/lib.rs`, 2000-char cap, levels info/warn/error).
//
// #362 introduced the seam for one narrow case: a `Channel.onmessage`
// throw that would otherwise wedge silently (see `harnessEvents.ts`'s
// `guardChannelHandler`). #404 widens it to the #238 seam's
// `sendPrompt` decisions and the mail-delivery decisions in
// `useMailDelivery.ts` — a devtools console is lost the moment a
// release build's window closes, so a lost first delivery could only
// ever be inferred after the fact, never reconstructed. `skein.log`
// survives that.
//
// Deliberately imports nothing but `@tauri-apps/api/core` — this
// module is imported from `harnessEvents.ts`, `harnessActivity.ts`,
// `harnessActivityCore.ts`, `harnessInput.ts` and `useMailDelivery.ts`,
// so pulling in anything from this codebase here risks an import
// cycle.

import { invoke } from "@tauri-apps/api/core";

/// Forward one log line to Rust. Never throws or rejects visibly — a
/// logging call must not become a new failure mode for the thing it's
/// trying to make visible. The `try`/`catch` guards a SYNCHRONOUS throw
/// from `invoke` itself, not just a rejected promise: under vitest's
/// node environment there is no Tauri runtime to answer the IPC call,
/// and some mocks throw synchronously rather than returning a rejected
/// promise.
export function logToRust(level: "info" | "warn" | "error", target: string, message: string): void {
	try {
		void invoke("frontend_log", { level, target, message }).catch(() => {});
	} catch {
		// No Tauri runtime, or invoke itself threw synchronously — swallow,
		// same as the `.catch(() => {})` above.
	}
}

/// Write `message` to the devtools console AND forward it to Rust's
/// `skein.log` via `logToRust` — the console is lost in a release
/// build, so anything worth reconstructing a lost delivery from has to
/// reach the file too. `message` is used as-is for both: callers
/// already write their own `[skein] ...` prefix, so this adds none of
/// its own.
export function logBoth(level: "info" | "warn" | "error", target: string, message: string): void {
	if (level === "warn") console.warn(message);
	else if (level === "error") console.error(message);
	else console.info(message);
	logToRust(level, target, message);
}
