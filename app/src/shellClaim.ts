// #318 — which Claude session a pane's POST-EXIT SHELL is bound to.
// After `/exit` + Enter the pane runs a shell; a `claude` typed there
// loads Skein's plugin through an env var, so its hooks POST under the
// pane's harness id. A NESTED claude (`claude -p` from a tool) fires the
// same hooks with the same harness id, so the id alone proves nothing.
// Pure and separate from `harnessActivity.ts`, like `sessionTracking.ts`.
//
// "Claim + session gate", per pane in shell mode:
//   - the FIRST SessionStart with source startup|resume|clear|fork and a
//     valid id CLAIMS the pane: children can only start after the outer
//     claude, so the first one is the shell's direct child. A claim sets
//     a one-shot fresh-process flag (the claimed claude is a new process,
//     which #336's resume handling needs to know).
//   - while claimed, a different-id `startup` (whatever source the claim
//     came from) gets probe-then-replace: it replaces the claim when
//     EITHER the claimed transcript is missing (phantom,
//     anthropics/claude-code#78455) OR the claimed harness is not
//     mid-turn (`shouldReplaceClaim`). A nested child can only be started
//     by the claimed claude while it is working (a tool call is in flight,
//     or a background task/subagent is outstanding and the end-of-turn
//     deferral keeps it `running`), so a startup landing while the
//     harness is idle/waiting is a new top-level claude typed in the
//     shell. This matters because SessionEnd is an async hook and can be
//     lost, which would otherwise ignore every later claude forever. The
//     caller supplies `claimedBusy` and does the probe; this module does
//     no IO. Anything else ignored while claimed is logged by the caller.
//   - `clear`/`resume`/`fork` with a different id are the claimed
//     claude's own and MOVE the claim (a nested `--resume` is an
//     accepted residual risk).
//   - SessionEnd for the claimed id releases the claim so the next claude
//     typed in the same shell can claim — except reason `clear`/`resume`,
//     which are followed by a SessionStart for the new id. Other ids are
//     ignored.
//   - permission pings: accepted only when claimed and the id matches;
//     a missing id while claimed is accepted (older payload; fail-open is
//     the safe direction for permission); unclaimed is rejected.
// Outside shell mode everything defers to `followedSession` / accepts.

import { followedSession } from "./sessionTracking";

export type ClaimSource = "startup" | "resume" | "clear" | "fork";

export type ShellDecision =
	| { kind: "ignore" }
	/** Follow `sessionId`. `claim`: this made/moved a shell claim. `repoint`:
	 * false when it equals the stored id (e.g. `claude --resume <same>`). */
	| { kind: "follow"; sessionId: string; source: string; claim: boolean; repoint: boolean }
	/** Caller probes `claude_session_exists(claimedId)` (only needed when
	 * `claimedBusy`), asks `shouldReplaceClaim(exists, claimedBusy)`, and if
	 * true calls `replaceClaim(harnessId, sessionId)` and follows `sessionId`. */
	| { kind: "probe-then-replace"; claimedId: string; sessionId: string; claimedBusy: boolean };

type Payload = { sessionId?: string | null; source?: string | null };
type Claim = { sessionId: string; source: ClaimSource };
type ShellState = { claim: Claim | null; fresh: boolean };

const CLAIMING: ReadonlySet<string> = new Set(["startup", "resume", "clear", "fork"]);

const shells = new Map<string, ShellState>();

/// Replace the claim iff the claimed transcript is a phantom or the
/// claimed harness is idle (see the header).
export function shouldReplaceClaim(claimedExists: boolean, claimedBusy: boolean): boolean {
	return !claimedExists || !claimedBusy;
}

/// Pure decision. `shell` null = not in shell mode. `claimedBusy`: the
/// claimed harness is mid-turn (running/permission or outstanding work).
export function decideSessionStart(
	shell: { claim: Claim | null } | null,
	current: string | undefined,
	payload: Payload,
	claimedBusy = false,
): ShellDecision {
	if (shell === null) {
		const f = followedSession(current, payload);
		return f === null ? { kind: "ignore" } : { kind: "follow", ...f, claim: false, repoint: true };
	}
	const { sessionId, source } = payload;
	if (typeof sessionId !== "string" || sessionId.length === 0) return { kind: "ignore" };
	if (typeof source !== "string" || !CLAIMING.has(source)) return { kind: "ignore" };
	const { claim } = shell;
	if (claim === null) {
		return { kind: "follow", sessionId, source, claim: true, repoint: sessionId !== current };
	}
	if (sessionId === claim.sessionId) return { kind: "ignore" };
	if (source === "startup") {
		return { kind: "probe-then-replace", claimedId: claim.sessionId, sessionId, claimedBusy };
	}
	return { kind: "follow", sessionId, source, claim: true, repoint: sessionId !== current };
}

/// Whether `argv0` runs the kind's own program (`claude`), by file stem
/// like the Rust side: `/usr/bin/claude`, `claude.exe` and `CLAUDE.CMD`
/// all match; a shell does not.
export function runsKindProgram(argv0: string | undefined, program: string | null): boolean {
	if (argv0 === undefined || program === null) return false;
	const base = argv0.split(/[\\/]/).pop() ?? "";
	const stem = base.replace(/\.(exe|cmd|bat|com)$/i, "");
	return stem.toLowerCase() === program.toLowerCase();
}

export const shellClaim = {
	isShell(id: string): boolean {
		return shells.has(id);
	},
	/// Decides, and records a claim/move when the decision says `claim`.
	onSessionStart(
		id: string,
		current: string | undefined,
		payload: Payload,
		claimedBusy = false,
	): ShellDecision {
		const state = shells.get(id) ?? null;
		const d = decideSessionStart(state, current, payload, claimedBusy);
		if (state !== null && d.kind === "follow" && d.claim) {
			// Fresh is armed only on unclaimed -> claimed (and in replaceClaim).
			// clear/resume/fork moves are the same process, so they don't re-arm.
			if (state.claim === null) state.fresh = true;
			state.claim = { sessionId: d.sessionId, source: d.source as ClaimSource };
		}
		return d;
	},
	/// The claim was a phantom or stale: a new process owns it. Re-arms the
	/// fresh flag (the phantom claim already consumed it on its re-point).
	replaceClaim(id: string, sessionId: string): void {
		const state = shells.get(id);
		if (!state) return;
		state.claim = { sessionId, source: "startup" };
		state.fresh = true;
	},
	/// The claimed session id, or null (unclaimed / not in shell mode).
	claimedId(id: string): string | null {
		return shells.get(id)?.claim?.sessionId ?? null;
	},
	/// True when the claim was released (the pane is unclaimed again).
	onSessionEnd(
		id: string,
		payload: { sessionId?: string | null; reason?: string | null },
	): boolean {
		const state = shells.get(id);
		if (!state?.claim || payload.sessionId !== state.claim.sessionId) return false;
		if (payload.reason === "clear" || payload.reason === "resume") return false;
		state.claim = null;
		return true;
	},
	acceptsPermission(id: string, sessionId: string | null | undefined): boolean {
		const state = shells.get(id);
		if (!state) return true;
		if (!state.claim) return false;
		if (typeof sessionId !== "string" || sessionId.length === 0) return true;
		return sessionId === state.claim.sessionId;
	},
	/// One-shot: true once after a claim, then false.
	consumeFreshProcess(id: string): boolean {
		const state = shells.get(id);
		if (!state?.fresh) return false;
		state.fresh = false;
		return true;
	},
	/// A spawn for `id`: a claude pane running its own program leaves shell
	/// mode, anything else (the post-exit shell) enters it. Other kinds: no-op.
	noteSpawn(id: string, kind: string, argv0: string | undefined): void {
		if (kind !== "claude") return;
		if (runsKindProgram(argv0, "claude")) shells.delete(id);
		else shells.set(id, { claim: null, fresh: false });
	},
	forget(id: string): void {
		shells.delete(id);
	},
};
