// harnessInputGate — the pure safety gates over the #238 nudge seam.
//
// Two pure gates over harness capabilities, activity phase and
// registration: `canSendPrompt` (paste + submit, end-of-turn only)
// and `canInsertText` (paste only, allowed mid-turn — #41). "No
// nudge where safety can't be proven": every refusal comes back as
// a reason string a caller can show verbatim as a disabled-button
// tooltip or a drag-overlay label, never a silent no-op.

import type { HarnessCapabilities } from "./data.tsx";
import type { HarnessActivity } from "./harnessActivity.ts";
import { atSafeStoppingPoint } from "./harnessActivity.ts";
import { launchSettled } from "./launchReady.ts";

/// Everything `canSendPrompt` needs, gathered by the caller so the gate
/// itself stays pure and DOM-free — testable in node with no xterm
/// instance and no store singleton.
export interface CanSendPromptInput {
	capabilities: HarnessCapabilities;
	activity: HarnessActivity | null;
	registered: boolean;
	/// Only consulted when `body` contains a newline.
	bracketedPasteOn: boolean;
	body: string;
	/// The current time, for the #404 launch-settle check below. Optional
	/// — defaults to `Date.now()` — so the existing callers in
	/// `review/ReviewPane.tsx` and `HarnessActionsMenu.tsx` (out of scope
	/// here) keep compiling and behaving exactly as before without
	/// passing it.
	nowMs?: number;
}

/// `draftHeld` (#413) is set only by the #383 composer-draft guard.
export type GateResult = { ok: true } | { ok: false; reason: string; draftHeld?: true };

/// Shared across `canSendPrompt` and `canInsertText`: the harness has to
/// have a terminal at all, and that terminal has to be the one the seam
/// actually knows about — a kind with no `pty` capability (`files`), a
/// harness that hasn't registered yet (just spawned, or just exited),
/// or one with no activity record at all (same two cases, seen from the
/// store side) are refused identically by both gates.
function checkRegistered(input: {
	capabilities: HarnessCapabilities;
	activity: HarnessActivity | null;
	registered: boolean;
}): GateResult | null {
	if (!input.capabilities.pty) {
		return { ok: false, reason: "this harness has no terminal to paste into" };
	}
	if (!input.registered || !input.activity) {
		return { ok: false, reason: "this harness's terminal isn't ready yet" };
	}
	return null;
}

/// Shared across `canSendPrompt` and `canInsertText`: once the phase
/// itself has cleared each gate's own check, both require the same
/// proof that something is actually watching this harness — an L2c
/// adapter attached and not given up on (`authoritative &&
/// !adapterSilent`), with proof of life from EITHER the tail
/// (`adapterHeard`) OR the harness's own launch signal
/// (`launchSignalAt !== null`, #273) — and that #215's config injection
/// happened for this spawn (`injected`), since without it there's no
/// evidence the harness's CLI even has the review tools wired up.
///
/// A launch signal is accepted as equivalent proof to `adapterHeard`
/// here, not a weaker substitute: it only ever arrives once the CLI is
/// past its trust gate and sitting at its own input prompt (verified
/// live — with the dialog open and unanswered, the hook does not fire),
/// and it is delivered by the very same #215 injection `injected`
/// already requires — the same channel that wires up the review MCP
/// tools is the one reporting the launch. A freshly spawned harness
/// that has only just cleared its trust dialog has no transcript yet
/// (Claude writes none until the first prompt), so `adapterHeard` alone
/// would keep the nudge gate refusing a harness that is, in fact, ready
/// and safe to paste into.
function checkWatched(activity: HarnessActivity): GateResult | null {
	if (
		!activity.authoritative ||
		(!activity.adapterHeard && activity.launchSignalAt === null) ||
		activity.adapterSilent
	) {
		return { ok: false, reason: "no confirmed adapter is watching this harness" };
	}
	if (!activity.injected) {
		return { ok: false, reason: "this harness wasn't started with the review tools wired up" };
	}
	return null;
}

/// Pure safety gate. Allowed only when every one of these holds:
///
///  - the harness kind has a terminal (`capabilities.pty`);
///  - it is registered in the seam (a live PTY, not a Files body or a
///    just-exited one);
///  - it is at a safe stopping point (`atSafeStoppingPoint`, #381): phase
///    `waiting` — end of turn, the moment a paste is unambiguously safe
///    to submit — or phase `running` with an armed #277 delegation
///    deferral, where the main session already ended its own turn and
///    only stayed `running` because background subagents were still
///    working. `permission` gets its own reason: it is a harder stop
///    than "not at a stopping point", not a variant of it, and outranks
///    the deferral too — `atSafeStoppingPoint` never treats a
///    `permission` phase as safe even with a deferral armed underneath;
///  - an L2c adapter is attached, has not gone silent, and has proven
///    itself either by speaking or by the harness's own launch signal
///    (`authoritative && (adapterHeard || launchSignalAt !== null) &&
///    !adapterSilent`, #273) — a `waiting` read off the L2a heuristic
///    alone is a guess, not proof;
///  - #215's config injection happened for this spawn (`injected`) —
///    without it there is no evidence the harness's CLI even has the
///    review tools wired up;
///  - #404: if this harness's only proof of life is the launch signal
///    (no transcript has spoken yet), its PTY must also have been quiet
///    for `LAUNCH_QUIET_MS` since that signal (or since spawn's own
///    launch ping, whichever cleared the cap first) — the launch ping
///    fires before the CLI is provably able to accept a paste, so a
///    harness whose terminal is still repainting refuses until it
///    settles or the `LAUNCH_READY_CAP_MS` cap passes. See
///    `launchReady.ts`;
///  - and, when `body` is multi-line, the terminal's own bracketed-paste
///    mode is on — otherwise a multi-line paste can arrive as several
///    separate "lines", each read as its own Enter.
export function canSendPrompt(input: CanSendPromptInput): GateResult {
	const { capabilities, activity, registered, bracketedPasteOn, body, nowMs } = input;
	const notReady = checkRegistered({ capabilities, activity, registered });
	if (notReady) return notReady;
	// `activity` is non-null here — `checkRegistered` above refused
	// otherwise.
	const a = activity as HarnessActivity;
	if (a.phase === "permission") {
		return { ok: false, reason: "the harness is waiting on a permission dialog" };
	}
	if (!atSafeStoppingPoint(a)) {
		return {
			ok: false,
			reason: `the harness isn't at a safe stopping point (currently ${a.phase})`,
		};
	}
	const notWatched = checkWatched(a);
	if (notWatched) return notWatched;
	const settlement = launchSettled({
		adapterHeard: a.adapterHeard,
		launchSignalAt: a.launchSignalAt,
		lastOutputAt: a.lastOutputAt,
		nowMs: nowMs ?? Date.now(),
	});
	if (!settlement.settled) {
		return { ok: false, reason: "the harness is still settling after launch (#404)" };
	}
	if (body.includes("\n") && !bracketedPasteOn) {
		return {
			ok: false,
			reason:
				"this harness's terminal isn't in paste mode, so a multi-line prompt isn't safe to send",
		};
	}
	return { ok: true };
}

/// Everything `canInsertText` needs, gathered by the caller for the same
/// reason `CanSendPromptInput` is — pure and DOM-free, testable with no
/// store singleton and no xterm instance.
export interface CanInsertTextInput {
	capabilities: HarnessCapabilities;
	activity: HarnessActivity | null;
	registered: boolean;
}

/// Pure safety gate for #41's drop-a-file seam — a paste with no
/// submit, so it is allowed at points `canSendPrompt` refuses: mid-turn,
/// while the harness is still working, not only at the end of one. Only
/// the phase check differs from `canSendPrompt`:
///
///  - `running`, `idle` and `waiting` are all fine — there's no "unsafe
///    to paste a path" moment among them, unlike a body that might get
///    submitted;
///  - `permission` gets its own reason, same principle as
///    `canSendPrompt`, but a sharper one: a path *typed* into an open
///    permission dialog could answer it, which a paste-without-submit
///    still risks becoming once the user's next keystroke lands;
///  - `spawning` and `exited` refuse too — there is no terminal on the
///    other end of the paste yet, or any more.
///
/// Registration, the adapter-watching proof (`authoritative &&
/// (adapterHeard || launchSignalAt !== null) && !adapterSilent`, #273),
/// and #215 injection are checked exactly as `canSendPrompt` checks
/// them — see `checkRegistered` / `checkWatched`.
export function canInsertText(input: CanInsertTextInput): GateResult {
	const { capabilities, activity, registered } = input;
	const notReady = checkRegistered({ capabilities, activity, registered });
	if (notReady) return notReady;
	const a = activity as HarnessActivity;
	if (a.phase === "permission") {
		return {
			ok: false,
			reason: "the harness is waiting on a permission dialog — a dropped path could answer it",
		};
	}
	if (a.phase === "spawning" || a.phase === "exited") {
		return {
			ok: false,
			reason: `the harness isn't ready to receive input (currently ${a.phase})`,
		};
	}
	return checkWatched(a) ?? { ok: true };
}

/// Wrap every path in `paths` that contains whitespace in double quotes,
/// join with single spaces, and add one trailing space so the user can
/// keep typing right after the drop lands. `[]` returns `""` — nothing
/// to insert, nothing to seed the input with.
export function formatDroppedPaths(paths: string[]): string {
	if (paths.length === 0) return "";
	const quoted = paths.map((p) => (/\s/.test(p) ? `"${p}"` : p));
	return `${quoted.join(" ")} `;
}
