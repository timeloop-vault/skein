// harnessInput — the seam for putting text into a harness's terminal
// and submitting it, plus the safety gate over doing so (#238).
//
// #238's Nudge button is the first caller; #41 (drop a file onto a
// harness) reuses the same registry rather than inventing its own way
// to reach a live PTY, but pairs it with its own gate — a drop has no
// submit, so it is safe at points a nudge is not.
//
// Two halves:
//
//   - A per-harness registry of `{ paste, bracketedPaste, submit }`,
//     filled in by `LiveTerminal` once its PTY is live and drained on
//     exit/unmount/respawn. Nothing outside `LiveTerminal` touches
//     xterm or `pty_write` for this — the registry is the only door.
//   - Two pure gates over harness capabilities, activity phase and
//     registration: `canSendPrompt` (paste + submit, end-of-turn only)
//     and `canInsertText` (paste only, allowed mid-turn — #41). "No
//     nudge where safety can't be proven": every refusal comes back as
//     a reason string a caller can show verbatim as a disabled-button
//     tooltip or a drag-overlay label, never a silent no-op.
//
// `submit` is a *separate* write from `paste` — the seam pastes the
// body (through xterm's `term.paste()`, so a multi-line body arrives as
// one bracketed-paste message rather than as keystrokes) and then
// writes a bare "\r", exactly the two actions a human would perform.
//
// #380: that submit is no longer written in the same tick as the
// paste. Claude Code 2.1.283 can drop the Enter ending a *first*
// bracketed paste into a freshly spawned harness (startup timing;
// upstream anthropics/claude-code#91205) — `sendPrompt` now pastes,
// waits `SUBMIT_GAP_MS`, then submits, skipping the submit outright
// (logged, text left unsent) if the target, the user, or the phase
// changed in that gap. For kinds that opt in
// (`capabilities.submitRetry`, Claude only), it then watches for
// `SUBMIT_RETRY_AFTER_MS` and fires one extra "\r" if nothing proves
// the first one landed. See `submitRetry.ts` for the pure policy
// underneath both waits.
//
// `sendPrompt` re-evaluates the gate at call time rather than trusting
// a value a caller rendered a moment earlier — the phase can flip
// between a render and the click that follows it. It also arms the
// #259 silent-adapter watchdog (#363) the same way a typed Enter does
// — this seam's submit never reaches xterm's `onKey`, so without it a
// harness prompted only through here (create_room, a mail nudge) would
// never fall back to the idle heuristic if its adapter stayed silent.

import { HARNESS_KINDS } from "./data.tsx";
import type { HarnessCapabilities } from "./data.tsx";
import { harnessActivity } from "./harnessActivity.ts";
import type { HarnessActivity } from "./harnessActivity.ts";
import { SUBMIT_GAP_MS, SUBMIT_RETRY_AFTER_MS, decideSubmitRetry } from "./submitRetry.ts";
import type { HarnessKind } from "./types.ts";

/// What a live harness's terminal offers this seam. `LiveTerminal` is
/// the only implementer today.
export interface HarnessInputTarget {
	/// Paste `text` into the terminal via xterm's own paste path
	/// (`term.paste`), not `onData` — the point is one bracketed-paste
	/// message, not a stream of synthetic keystrokes.
	paste(text: string): void;
	/// Is the terminal's own DECSET 2004 paste mode currently on? Only
	/// xterm knows this (`term.modes.bracketedPasteMode`) — it depends
	/// on what the child enabled.
	bracketedPaste(): boolean;
	/// Submit whatever is now in the harness's input — a bare "\r",
	/// written as its own `pty_write` call after the paste.
	submit(): void;
}

const targets = new Map<string, HarnessInputTarget>();

/// #380: per-harness "did the user type anything" counter, bumped by
/// `noteUserInput`. A counter rather than a timestamp — `sendPrompt`'s
/// gap and retry checks only ever ask "has this moved since I
/// snapshotted it", never "when", so there's no clock to keep in sync.
/// Never cleared on unregister: a stale, never-again-read entry for a
/// harness id that won't be reused costs one map slot, which is cheaper
/// than adding a leak-prone lifecycle hook here for it.
const userInputCounts = new Map<string, number>();

/// Internal to this module — only `sendPrompt`'s gap/retry checks need
/// the current count, by comparing a snapshot taken at paste/submit
/// time against the value now.
function userInputCount(id: string): number {
	return userInputCounts.get(id) ?? 0;
}

export const harnessInput = {
	/// Publish `target` for `id`. Returns the unregister function —
	/// call it on PTY exit, on unmount, and before a respawn re-registers
	/// under the same harness id, so a dead or about-to-change terminal
	/// is never still a nudge target.
	register(id: string, target: HarnessInputTarget): () => void {
		targets.set(id, target);
		return () => {
			// Only clear our own registration: a respawn that already
			// registered a fresh target for the same id must not have
			// its target ripped out by the old effect's belated cleanup.
			if (targets.get(id) === target) targets.delete(id);
		};
	},
	isRegistered(id: string): boolean {
		return targets.has(id);
	},
	bracketedPaste(id: string): boolean {
		return targets.get(id)?.bracketedPaste() ?? false;
	},
	/// Record that the user typed (or pasted) into `id`'s terminal just
	/// now (#380). Called from `useTerminalSpawn`'s `term.onKey` handler
	/// next to `recordInput`, and from the terminal's own Ctrl+V/
	/// Ctrl+Shift+V paste path in `terminalInteractions.ts` — both are a
	/// human driving the terminal, which `sendPrompt`'s gap and retry
	/// checks need to tell apart from its own machine-written "\r".
	noteUserInput(id: string): void {
		userInputCounts.set(id, userInputCount(id) + 1);
	},
};

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
}

export type GateResult = { ok: true } | { ok: false; reason: string };

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
///  - its phase is `waiting` — end of turn, the one moment a paste is
///    unambiguously safe to submit. `permission` gets its own reason:
///    it is a harder stop than "not waiting", not a variant of it;
///  - an L2c adapter is attached, has not gone silent, and has proven
///    itself either by speaking or by the harness's own launch signal
///    (`authoritative && (adapterHeard || launchSignalAt !== null) &&
///    !adapterSilent`, #273) — a `waiting` read off the L2a heuristic
///    alone is a guess, not proof;
///  - #215's config injection happened for this spawn (`injected`) —
///    without it there is no evidence the harness's CLI even has the
///    review tools wired up;
///  - and, when `body` is multi-line, the terminal's own bracketed-paste
///    mode is on — otherwise a multi-line paste can arrive as several
///    separate "lines", each read as its own Enter.
export function canSendPrompt(input: CanSendPromptInput): GateResult {
	const { capabilities, activity, registered, bracketedPasteOn, body } = input;
	const notReady = checkRegistered({ capabilities, activity, registered });
	if (notReady) return notReady;
	// `activity` is non-null here — `checkRegistered` above refused
	// otherwise.
	const a = activity as HarnessActivity;
	if (a.phase === "permission") {
		return { ok: false, reason: "the harness is waiting on a permission dialog" };
	}
	if (a.phase !== "waiting") {
		return {
			ok: false,
			reason: `the harness isn't at a safe stopping point (currently ${a.phase})`,
		};
	}
	const notWatched = checkWatched(a);
	if (notWatched) return notWatched;
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

/// Paste `body` into `harnessId`'s terminal and submit it, after
/// re-checking `canSendPrompt` against the *current* state — never the
/// state a caller rendered a moment ago. Returns the gate result either
/// way, so a caller that raced a phase change can show why it refused.
///
/// #380: the submit doesn't land in the same tick as the paste — see
/// this file's header comment and `submitRetry.ts`. The paste and the
/// gate result are still synchronous; only the "\r" (and, for a
/// retry-capable kind, its possible retry) happen later, off a timer.
export function sendPrompt(harnessId: string, kind: HarnessKind, body: string): GateResult {
	const target = targets.get(harnessId);
	const gate = canSendPrompt({
		capabilities: HARNESS_KINDS[kind].capabilities,
		activity: harnessActivity.get(harnessId),
		registered: target !== undefined,
		bracketedPasteOn: target?.bracketedPaste() ?? false,
		body,
	});
	if (!gate.ok) return gate;
	// A passing gate required `registered: target !== undefined`, so
	// `target` is set here by construction.
	const t = target as HarnessInputTarget;
	t.paste(body);

	const inputCountAtPaste = userInputCount(harnessId);
	setTimeout(() => {
		// Skip the submit outright — leaving the pasted text unsent
		// rather than risk it — if anything about the target changed in
		// the gap: a respawn (registration moved on), the user now
		// typing (a machine "\r" landing on top of a human's own input
		// is never safe), or the phase having moved off `waiting` at
		// all. That last check mirrors `canSendPrompt` itself rather
		// than singling out `permission` — a blind "\r" is just as
		// wrong after any other phase drift (the turn ended and started
		// a new one, the harness exited) as it is into an open dialog;
		// `permission` only gets its own message because it's the
		// common, nameable case (#86's territory, not this one's).
		if (targets.get(harnessId) !== t) {
			console.warn(
				`[skein] sendPrompt: harness ${harnessId}'s terminal changed before the submit — leaving the paste unsent (#380)`,
			);
			return;
		}
		if (userInputCount(harnessId) !== inputCountAtPaste) {
			console.warn(
				`[skein] sendPrompt: user typed into harness ${harnessId} before the submit — leaving the paste unsent (#380)`,
			);
			return;
		}
		const phaseAtSubmit = harnessActivity.get(harnessId)?.phase;
		if (phaseAtSubmit !== "waiting") {
			const why =
				phaseAtSubmit === "permission"
					? "opened a permission dialog"
					: `moved to ${phaseAtSubmit ?? "no activity record"}`;
			console.warn(
				`[skein] sendPrompt: harness ${harnessId} ${why} before the submit — leaving the paste unsent (#380)`,
			);
			return;
		}
		t.submit();
		// #363: this submit never touches xterm's `onKey`, so without this
		// call the #259 silent-adapter watchdog would never arm for a prompt
		// that arrived through this seam — `create_room`'s first prompt, a
		// mail nudge.
		harnessActivity.notePromptSubmitted(harnessId);

		// #380: one retry, only for kinds that opt in (Claude). Watches
		// for proof the first Enter landed — any transition out of
		// `waiting` — for `SUBMIT_RETRY_AFTER_MS`, well inside the #259
		// watchdog's own `ADAPTER_SILENT_AFTER_MS` window.
		if (!HARNESS_KINDS[kind].capabilities.submitRetry) return;
		const inputCountAtSubmit = userInputCount(harnessId);
		let leftWaitingSinceSend = false;
		// Deliberately not tied to this harness's unmount/respawn: it
		// self-cleans below after `SUBMIT_RETRY_AFTER_MS` regardless of
		// what happens to the harness in between, and a target that goes
		// away or respawns in the meantime is caught by
		// `decideSubmitRetry`'s own `sameTarget` check (and a harness
		// that fully exits shows up there as `phase: null`) — no extra
		// unsubscribe-on-teardown path needed.
		const unsubscribe = harnessActivity.subscribeTransitions((id, from) => {
			if (id === harnessId && from === "waiting") leftWaitingSinceSend = true;
		});
		setTimeout(() => {
			unsubscribe();
			const decision = decideSubmitRetry({
				capable: true,
				leftWaitingSinceSend,
				phase: harnessActivity.get(harnessId)?.phase ?? null,
				userInputSinceSend: userInputCount(harnessId) !== inputCountAtSubmit,
				sameTarget: targets.get(harnessId) === t,
			});
			if (!decision.retry) return;
			console.info(
				`[skein] sendPrompt: retrying the submit into harness ${harnessId} — the first Enter didn't appear to land (#380)`,
			);
			t.submit();
		}, SUBMIT_RETRY_AFTER_MS);
	}, SUBMIT_GAP_MS);

	return gate;
}

/// Paste `text` into `harnessId`'s terminal — and only that, never
/// `submit` — after re-checking `canInsertText` against the *current*
/// state. #41's drop-a-file caller: a path dropped mid-turn seeds the
/// input for whatever the user types next, it does not send anything on
/// the harness's behalf. Same re-check-at-call-time contract as
/// `sendPrompt`.
export function insertText(harnessId: string, kind: HarnessKind, text: string): GateResult {
	const target = targets.get(harnessId);
	const gate = canInsertText({
		capabilities: HARNESS_KINDS[kind].capabilities,
		activity: harnessActivity.get(harnessId),
		registered: target !== undefined,
	});
	if (!gate.ok) return gate;
	// A passing gate required `registered: target !== undefined`, so
	// `target` is set here by construction.
	target?.paste(text);
	return gate;
}
