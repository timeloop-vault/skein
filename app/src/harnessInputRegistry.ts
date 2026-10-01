// harnessInputRegistry — the per-harness registry half of the #238 nudge
// seam (see `harnessInput.ts` for the public entry that re-exports it).
//
// A per-harness registry of `{ paste, bracketedPaste, submit }`, filled
// in by `LiveTerminal` once its PTY is live and drained on
// exit/unmount/respawn. Nothing outside `LiveTerminal` touches xterm or
// `pty_write` for this — the registry is the only door.
//
// #383: a per-harness `ComposerDraft` (`composerDraft.ts`) rides beside
// `userInputCounts` below, fed by every place text can land in a
// terminal — `useTerminalSpawn.ts`'s `onKey`, its bracketed-paste
// branch in `onData`, and its `compositionstart`/`compositionend`
// listeners on the terminal's textarea (IME composition — CJK, an
// emoji picker, possibly a dead-key accent — calls xterm's own
// `_finalizeComposition` straight into `onData` and never reaches
// `onKey`, so this is the only place it's seen at all; both events
// fold to `unknown`, same fail-safe direction as everything else here),
// `terminalInteractions.ts`'s Ctrl+V and Shift/Alt+Enter paths, and
// `harnessInputSend.ts`'s own seam paste/submit. An AUTOMATIC send
// (`sendPrompt`'s `opts.automatic`) folds `checkDraft` on top of
// `canSendPrompt`, so a mail nudge holds rather than pastes over
// whatever the user is mid-typing; a manual send never consults it.
// `useMailDelivery.ts` also fires a retry the moment a draft clears
// (`subscribeDraftCleared`), so held mail doesn't wait for the next
// unrelated trigger. What this can't see, by the header comment on
// `composerDraft.ts`: text the CLI itself restores without any key or
// paste passing through Skein's terminal at all — a resumed session's
// own remembered input, a draft opencode/Claude keeps across `/clear`,
// a message the CLI queues internally — and a paste that reaches
// neither `onKey` nor the bracketed-paste branch of `onData`.

import { CLEAN_DRAFT, describeDraftEvent, draftClearedBy, reduceDraft } from "./composerDraft.ts";
import type { ComposerDraft, DraftEvent } from "./composerDraft.ts";
import { logBoth } from "./frontendLog.ts";
import { readComposer } from "./promptScreen.ts";
import type { ComposerReading, ScreenSnapshot } from "./promptScreen.ts";
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
	/// #413: which harness kind this terminal runs — `readComposer` needs it
	/// to pick the right prompt recogniser. Absent = never screen-checked.
	kind?: HarnessKind;
	/// #413: a snapshot of the visible screen (independent of user scroll),
	/// or null when it can't be read. Absent = never screen-checked.
	screen?(): ScreenSnapshot | null;
}

/// #413: how long after the user's last draft event a screen read may be
/// trusted. The PTY echo can lag the `onKey` event, so a snapshot taken too
/// soon still shows the pre-keystroke empty prompt — the one false "empty"
/// this check must avoid.
export const SCREEN_SETTLE_MS = 1500;

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

/// #383: per-harness inferred composer state, alongside `userInputCounts`
/// above — see `composerDraft.ts` for the state machine and this file's
/// header for how it's wired in. Missing entry reads as `CLEAN_DRAFT`,
/// same convention as `userInputCounts`' missing-entry-is-0.
const drafts = new Map<string, ComposerDraft>();

/// Subscribers notified once per transition `draftClearedBy` calls a
/// "cleared" — never on every event, so a caller doesn't have to
/// re-derive that predicate itself.
/// #413: why a draft is `unknown`, so the screen check can tell the user's
/// own doing from the seam's. A `seamPaste` also sets `unknown`, but the
/// screen then shows the NUDGE's pasted body, not the user's text — and its
/// own `seamSubmit` (or the user's next event) resolves that. Reading an
/// empty screen mid-flight (paste not yet echoed) must never release it, so
/// the check runs only for a "user"-caused unknown.
const unknownCause = new Map<string, "user" | "seam">();
/// #413: when the user last produced a draft event (key / userPaste).
const lastUserDraftEventAt = new Map<string, number>();
/// #413: per-harness debounced screen-check timers.
const screenTimers = new Map<string, ReturnType<typeof setTimeout>>();
/// #413: the event class and time that last moved a draft to typed/unknown.
const draftCauses = new Map<string, { event: string; at: number }>();
/// #413: last screen-check outcome logged per harness, to dedupe the timer.
const lastScreenLog = new Map<string, string>();

function cancelScreenTimer(id: string): void {
	const t = screenTimers.get(id);
	if (t !== undefined) clearTimeout(t);
	screenTimers.delete(id);
}

const draftClearedSubscribers = new Set<(id: string) => void>();

/// #386: subscribers notified once `register` has finished setting up
/// `id` — target published, respawn draft event noted. This is
/// `useMailDelivery.ts`'s seam-registered trigger: mail that landed
/// while a fresh harness was still `spawning` and missed the one
/// `spawning → waiting` check (the seam not registered yet at the
/// time) gets one more chance to be re-evaluated against the unchanged
/// send gate the moment the seam itself comes up.
const registeredSubscribers = new Set<(id: string) => void>();

export const harnessInput = {
	/// Publish `target` for `id`. Returns the unregister function —
	/// call it on PTY exit, on unmount, and before a respawn re-registers
	/// under the same harness id, so a dead or about-to-change terminal
	/// is never still a nudge target.
	register(id: string, target: HarnessInputTarget): () => void {
		targets.set(id, target);
		// #383: a fresh PTY means an empty composer, whatever the old one
		// last read as.
		harnessInput.noteDraftEvent(id, { type: "respawn" });
		// #386: notified AFTER the target is set and the respawn draft
		// event is noted, so a subscriber that immediately re-checks
		// `canSendPrompt`/`sendPrompt` sees this harness as already
		// registered and its draft already reset.
		for (const cb of registeredSubscribers) cb(id);
		return () => {
			// Only clear our own registration: a respawn that already
			// registered a fresh target for the same id must not have
			// its target ripped out by the old effect's belated cleanup.
			if (targets.get(id) === target) {
				targets.delete(id);
				cancelScreenTimer(id);
				unknownCause.delete(id);
				lastUserDraftEventAt.delete(id);
			}
		};
	},
	isRegistered(id: string): boolean {
		return targets.has(id);
	},
	/// The live target for `id`, or undefined — read accessor over the
	/// module-private `targets` map for `harnessInputSend.ts`, whose
	/// paste/submit mechanics must act on exactly the registered terminal.
	target(id: string): HarnessInputTarget | undefined {
		return targets.get(id);
	},
	/// #404: read accessor over the module-private `userInputCount` above
	/// — `useMailDelivery.ts` snapshots this at nudge-paste time and
	/// compares it again at rollback, the same "has this moved since I
	/// snapshotted it" pattern `sendPrompt`'s own gap/retry checks use, to
	/// tell whether a human drove the harness before deciding whether a
	/// lost-silence recovery is safe to attempt.
	userInputCount(id: string): number {
		return userInputCount(id);
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
	/// #383: fold `ev` into `id`'s inferred composer draft, and notify
	/// `subscribeDraftCleared` subscribers when `draftClearedBy` says this
	/// transition emptied it without a submit.
	noteDraftEvent(id: string, ev: DraftEvent, nowMs: number = Date.now()): void {
		const prev = drafts.get(id) ?? CLEAN_DRAFT;
		const next = reduceDraft(prev, ev);
		drafts.set(id, next);
		const userEvent = ev.type === "key" || ev.type === "userPaste";
		if (userEvent) lastUserDraftEventAt.set(id, nowMs);
		if (next.kind === "unknown") {
			if (ev.type === "seamPaste") unknownCause.set(id, "seam");
			else if (userEvent) unknownCause.set(id, "user");
		} else {
			unknownCause.delete(id);
		}
		if (prev.kind !== next.kind) {
			const cls = describeDraftEvent(ev);
			if (next.kind === "clean") draftCauses.delete(id);
			else draftCauses.set(id, { event: cls, at: nowMs });
			lastScreenLog.delete(id);
			logBoth(
				"debug",
				"skein::seam",
				`[skein] composerDraft: harness ${id} ${prev.kind} → ${next.kind} (${cls}) (#413)`,
			);
		}
		// Debounced: every user event that leaves the draft `unknown` pushes
		// the screen check out by another settle window, so "type, move the
		// cursor, erase it all" releases held mail without an Enter.
		if (ev.type === "seamPaste" || ev.type === "respawn" || next.kind !== "unknown") {
			cancelScreenTimer(id);
		} else if (userEvent) {
			cancelScreenTimer(id);
			screenTimers.set(
				id,
				setTimeout(() => {
					screenTimers.delete(id);
					harnessInput.checkScreen(id);
				}, SCREEN_SETTLE_MS),
			);
		}
		if (draftClearedBy(prev, ev, next)) {
			for (const cb of draftClearedSubscribers) cb(id);
		}
	},
	/// #413: when the draft is `unknown` because of the USER (not a seam
	/// paste), settled for `SCREEN_SETTLE_MS`, and the screen confidently
	/// reads an empty composer, fold `screenEmpty` (→ `clean`, which fires
	/// the draft-cleared subscribers). Returns true iff it did. Never throws.
	checkScreen(id: string, nowMs: number = Date.now()): boolean {
		if (harnessInput.draft(id).kind !== "unknown") return false;
		if (unknownCause.get(id) !== "user") return false;
		const target = targets.get(id);
		if (target?.screen === undefined || target.kind === undefined) return false;
		if (nowMs - (lastUserDraftEventAt.get(id) ?? Number.NEGATIVE_INFINITY) < SCREEN_SETTLE_MS) {
			return false;
		}
		let reading: "empty" | "text" | "unknown" = "unknown";
		try {
			const snap = target.screen();
			if (snap !== null) reading = readComposer(target.kind, snap);
		} catch {
			reading = "unknown";
		}
		const line =
			reading === "empty" ? "screen reads empty → clean" : `screen reads ${reading}, still held`;
		if (lastScreenLog.get(id) !== line) {
			lastScreenLog.set(id, line);
			logBoth("debug", "skein::seam", `[skein] composerDraft: harness ${id} ${line} (#413)`);
		}
		if (reading !== "empty") return false;
		harnessInput.noteDraftEvent(id, { type: "screenEmpty" }, nowMs);
		return true;
	},
	/// #413: the event class and time that last moved `id`'s draft to
	/// typed/unknown, or null while clean — never the typed content. For a
	/// later health view (#414) and the "nudge refused" log line.
	draftCause(id: string): { event: string; at: number } | null {
		return draftCauses.get(id) ?? null;
	},
	/// #413: a fresh screen reading right now, with NO settle window —
	/// for a user click ("deliver now"), where the PTY echo has had time
	/// to land. null when there is no target, screen or kind, or the read
	/// throws. Never mutates the draft.
	readComposerNow(id: string): ComposerReading | null {
		const target = targets.get(id);
		if (target?.screen === undefined || target.kind === undefined) return null;
		try {
			const snap = target.screen();
			return snap === null ? null : readComposer(target.kind, snap);
		} catch {
			return null;
		}
	},
	/// `id`'s current inferred composer draft — `CLEAN_DRAFT` for a
	/// harness this store has never seen an event for.
	draft(id: string): ComposerDraft {
		return drafts.get(id) ?? CLEAN_DRAFT;
	},
	/// Subscribe to "a harness's draft just cleared without a submit" —
	/// `useMailDelivery.ts`'s fourth trigger, so held mail retries the
	/// moment it's safe rather than waiting for an unrelated event.
	/// Returns the unsubscribe function.
	subscribeDraftCleared(cb: (id: string) => void): () => void {
		draftClearedSubscribers.add(cb);
		return () => {
			draftClearedSubscribers.delete(cb);
		};
	},
	/// #386: subscribe to "a harness just finished registering in this
	/// seam" — `useMailDelivery.ts`'s seam-registered trigger, so mail
	/// that landed too early (before this harness had a target to paste
	/// into at all) gets re-checked the moment one exists. Returns the
	/// unsubscribe function.
	subscribeRegistered(cb: (id: string) => void): () => void {
		registeredSubscribers.add(cb);
		return () => {
			registeredSubscribers.delete(cb);
		};
	},
};
