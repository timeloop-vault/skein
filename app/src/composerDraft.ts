// composerDraft — tracks whether a harness's composer holds the user's OWN
// unsent text (#383). An automatic mail nudge (#329/#381) must never paste
// on top of a draft the user is mid-typing: `harnessInput.sendPrompt`'s
// paste-then-submit would land inside whatever the user has queued up,
// corrupting both. This module is the pure state machine underneath that
// check — no DOM, no store, no xterm instance, so it is testable with a
// plain table of keys.
//
// The state is inferred, not observed: xterm's `term.onKey(({key}) => …)`
// (`useTerminalSpawn.ts`) hands over the exact byte string sent to the PTY,
// and that is all this module has to reconstruct "is there text sitting in
// the composer right now" from. IME composition (CJK, an emoji picker,
// possibly a dead-key accent) never reaches `onKey` at all — xterm's own
// `_finalizeComposition` writes straight to `onData` — so
// `useTerminalSpawn.ts` instead listens for `compositionstart`/
// `compositionend` on the terminal's textarea and folds both to
// `userPaste` (→ `unknown`): the same fail-safe direction as everything
// else here, since this module has no way to know what a composition
// actually committed. It deliberately errs toward `unknown` over
// `clean` whenever a key's effect on the composer can't be inferred purely
// from its own bytes (cursor movement, a word-delete chord, an unrecognised
// escape sequence) — a false "clean" is the dangerous direction, since it
// is exactly the reading that lets an automatic nudge paste over a draft;
// a false "unknown" only costs a held nudge until the next opportunity.
//
// `unknown` therefore also means "something MAY be there" for the purposes
// of the gate below, not "nothing is". `checkDraft` treats `typed` and
// `unknown` identically for that reason — the gate has no way to act on
// the distinction that would be safe.

/// A harness's composer, as best this module can infer it from the keys
/// and events it has seen since the terminal last came up clean.
export type ComposerDraft =
	| { kind: "clean" }
	| { kind: "typed"; chars: number } // chars > 0
	| { kind: "unknown" }; // something may be there; can't tell

export const CLEAN_DRAFT: ComposerDraft = { kind: "clean" };

/// What `reduceDraft` folds over. `key` events are exactly the strings
/// `useTerminalSpawn.ts`'s `term.onKey(({key}) => …)` hands the PTY — no
/// re-parsing, no re-encoding, so a fixture string here is a real key.
export type DraftEvent =
	| { type: "key"; key: string } // a user keystroke from onKey
	| { type: "userPaste" } // the user pasted (Ctrl+V path, or a bracketed paste seen in onData)
	| { type: "seamPaste" } // harnessInput.sendPrompt pasted a body
	| { type: "seamSubmit" } // harnessInput.sendPrompt wrote its "\r"
	| { type: "respawn" }; // a fresh PTY registered for this harness

const HISTORY_RECALL_KEYS = new Set([
	"\x1b[A", // Up
	"\x1bOA", // Up (application cursor mode)
	"\x1b[B", // Down
	"\x1bOB", // Down (application cursor mode)
	"\x12", // Ctrl+R
	"\x10", // Ctrl+P
	"\x0e", // Ctrl+N
]);

/// A key is "printable text" when it carries no control byte below 0x20
/// and isn't DEL (0x7f) — checked per code point, not per UTF-16 unit, so
/// a surrogate-pair emoji or an IME composition still counts as one
/// printable key rather than tripping on a lone surrogate. `\x1b\r`
/// (Shift/Alt+Enter, #27's newline-in-prompt) is handled by its own check
/// before this one runs, since escape (0x1b) would otherwise fail it.
function isPrintableKey(key: string): boolean {
	if (key.length === 0) return false;
	for (const ch of key) {
		const code = ch.codePointAt(0) ?? 0;
		if (code < 0x20 || code === 0x7f) return false;
	}
	return true;
}

/// The composer-relevant classification of one `onKey` string. Kept
/// separate from `reduceDraft`'s per-kind fold so each branch below reads
/// as one rule rather than a wall of string comparisons.
type KeyEffect =
	| "submit"
	| "clear"
	| "backspace"
	| "historyRecall"
	| "shiftEnter"
	| "printable"
	| "other";

function classifyKey(key: string): KeyEffect {
	if (key === "\r") return "submit";
	if (key === "\x03") return "clear"; // Ctrl+C — both Claude Code and opencode clear the input on it
	if (key === "\x7f" || key === "\b") return "backspace";
	if (HISTORY_RECALL_KEYS.has(key)) return "historyRecall";
	if (key === "\x1b\r") return "shiftEnter"; // Shift/Alt+Enter — a newline inserted into the prompt (#27)
	if (isPrintableKey(key)) return "printable";
	return "other";
}

/// Pure fold: `prev` plus one `ev` gives the next inferred composer state.
/// When a key's effect on the composer can't be inferred from its bytes
/// alone, the answer is `unknown`, never `clean` — see this file's header.
export function reduceDraft(prev: ComposerDraft, ev: DraftEvent): ComposerDraft {
	switch (ev.type) {
		case "userPaste":
			return { kind: "unknown" };
		case "seamPaste":
			// The nudge body now sits in the composer until its own submit
			// lands — held, not clean, so a second automatic send can't
			// stack on top of the first.
			return { kind: "unknown" };
		case "seamSubmit":
			return CLEAN_DRAFT;
		case "respawn":
			return CLEAN_DRAFT;
		case "key":
			return reduceKey(prev, ev.key);
	}
}

/// Shared by the `printable` and `shiftEnter` cases below: `len` code
/// points were inserted into the composer.
function insertPrintable(prev: ComposerDraft, len: number): ComposerDraft {
	if (prev.kind === "clean") return { kind: "typed", chars: len };
	if (prev.kind === "typed") return { kind: "typed", chars: prev.chars + len };
	return prev; // unknown → unknown
}

function reduceKey(prev: ComposerDraft, key: string): ComposerDraft {
	const effect = classifyKey(key);
	switch (effect) {
		case "submit":
			return CLEAN_DRAFT;
		case "clear":
			return CLEAN_DRAFT;
		case "backspace":
			if (prev.kind === "typed") {
				return prev.chars <= 1 ? CLEAN_DRAFT : { kind: "typed", chars: prev.chars - 1 };
			}
			return prev; // clean → clean, unknown → unknown
		case "historyRecall":
			// Recalling history from an empty composer puts text into it —
			// true from any prior state, since it overwrites whatever was
			// there.
			return { kind: "unknown" };
		case "shiftEnter":
			return insertPrintable(prev, 1);
		case "printable":
			return insertPrintable(prev, [...key].length);
		case "other":
			// Cursor movement, Home/End, Delete, Tab, Esc, Ctrl+W/Ctrl+U, or
			// any other control/escape sequence this module doesn't parse.
			// Nothing was inserted, so `clean` stays `clean` — but once
			// there's a count to track, a cursor move or word-delete makes
			// that count meaningless, so `typed` degrades to `unknown`
			// rather than risk it.
			if (prev.kind === "clean") return CLEAN_DRAFT;
			return { kind: "unknown" };
	}
}

/// True only for a transition that EMPTIED the composer without submitting
/// it — `prev` held something, `next` reads clean, and the event that got
/// it there was neither a submit (`"\r"` or `seamSubmit`) nor a `respawn`.
/// The caller fires a "draft cleared" trigger on this so held mail can be
/// delivered. A submit is excluded because the turn it starts is itself
/// the next trigger (running → waiting); delivering right after Enter,
/// before the adapter reports the turn, would paste on top of a prompt
/// the CLI is already processing. A respawn is excluded for the same
/// reason it resets to clean in the first place — a fresh PTY has no
/// draft to have "cleared" at all.
export function draftClearedBy(prev: ComposerDraft, ev: DraftEvent, next: ComposerDraft): boolean {
	if (prev.kind === "clean") return false;
	if (next.kind !== "clean") return false;
	if (ev.type === "respawn") return false;
	if (ev.type === "seamSubmit") return false;
	if (ev.type === "key" && ev.key === "\r") return false;
	return true;
}

/// The gate an AUTOMATIC send adds on top of `canSendPrompt` (#238):
/// `null` when the composer reads clean, a refusal for `typed` and
/// `unknown` alike — the gate has no way to act on the difference between
/// "definitely holds text" and "might", so both are held the same way.
export function checkDraft(draft: ComposerDraft): { ok: false; reason: string } | null {
	if (draft.kind === "clean") return null;
	return {
		ok: false,
		reason:
			"the composer holds unsent text — holding automatic delivery until it's submitted or cleared",
	};
}
