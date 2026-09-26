import { describe, expect, it } from "vitest";
import { CLEAN_DRAFT, checkDraft, draftClearedBy, reduceDraft } from "./composerDraft.ts";
import type { ComposerDraft, DraftEvent } from "./composerDraft.ts";

const clean = CLEAN_DRAFT;
const typed = (chars: number): ComposerDraft => ({ kind: "typed", chars });
const unknown: ComposerDraft = { kind: "unknown" };

const key = (k: string): DraftEvent => ({ type: "key", key: k });

describe("reduceDraft — key: submit and clear", () => {
	it.each<[string, ComposerDraft, string, ComposerDraft]>([
		["Enter on clean stays clean", clean, "\r", clean],
		["Enter on typed clears", typed(3), "\r", clean],
		["Enter on unknown clears", unknown, "\r", clean],
		["Ctrl+C on clean stays clean", clean, "\x03", clean],
		["Ctrl+C on typed clears", typed(2), "\x03", clean],
		["Ctrl+C on unknown clears", unknown, "\x03", clean],
	])("%s", (_desc, prev, k, expected) => {
		expect(reduceDraft(prev, key(k))).toEqual(expected);
	});
});

describe("reduceDraft — key: backspace", () => {
	it.each<[string, ComposerDraft, string, ComposerDraft]>([
		["backspace (DEL) on clean stays clean", clean, "\x7f", clean],
		["backspace (BS) on clean stays clean", clean, "\b", clean],
		["backspace on typed(1) clears", typed(1), "\x7f", clean],
		["backspace on typed(2) decrements", typed(2), "\x7f", typed(1)],
		["backspace (BS form) on typed(2) decrements", typed(2), "\b", typed(1)],
		["backspace on unknown stays unknown", unknown, "\x7f", unknown],
	])("%s", (_desc, prev, k, expected) => {
		expect(reduceDraft(prev, key(k))).toEqual(expected);
	});
});

describe("reduceDraft — key: history recall", () => {
	it.each<[string, string]>([
		["Up", "\x1b[A"],
		["Up (app cursor mode)", "\x1bOA"],
		["Down", "\x1b[B"],
		["Down (app cursor mode)", "\x1bOB"],
		["Ctrl+R", "\x12"],
		["Ctrl+P", "\x10"],
		["Ctrl+N", "\x0e"],
	])("%s always goes to unknown, from any state", (_desc, k) => {
		expect(reduceDraft(clean, key(k))).toEqual(unknown);
		expect(reduceDraft(typed(4), key(k))).toEqual(unknown);
		expect(reduceDraft(unknown, key(k))).toEqual(unknown);
	});
});

describe("reduceDraft — key: printable text", () => {
	it.each<[string, ComposerDraft, string, ComposerDraft]>([
		["single char from clean", clean, "a", typed(1)],
		["multi-char chunk from clean", clean, "ab", typed(2)],
		["single char from typed accumulates", typed(2), "c", typed(3)],
		["multi-char chunk from typed accumulates", typed(2), "xyz", typed(5)],
		["printable from unknown stays unknown", unknown, "a", unknown],
		// #27: Shift/Alt+Enter inserts a newline, counted as length 1.
		["Shift/Alt+Enter from clean counts as length 1", clean, "\x1b\r", typed(1)],
		["Shift/Alt+Enter from typed accumulates by 1", typed(2), "\x1b\r", typed(3)],
		// A surrogate-pair key delivered via `onKey` (not IME composition,
		// which never reaches `onKey` at all — see this file's header)
		// may be several UTF-16 units but one code point.
		["a surrogate-pair emoji counts as one printable char", clean, "😀", typed(1)],
	])("%s", (_desc, prev, k, expected) => {
		expect(reduceDraft(prev, key(k))).toEqual(expected);
	});
});

describe("reduceDraft — key: other (no inferable insertion)", () => {
	it.each<[string, string]>([
		["Left arrow", "\x1b[D"],
		["Right arrow", "\x1b[C"],
		["Home", "\x1b[H"],
		["End", "\x1b[F"],
		["Delete", "\x1b[3~"],
		["Tab", "\t"],
		["Esc", "\x1b"],
		["Ctrl+W", "\x17"],
		["Ctrl+U", "\x15"],
	])("%s: clean stays clean, typed degrades to unknown, unknown stays unknown", (_desc, k) => {
		expect(reduceDraft(clean, key(k))).toEqual(clean);
		expect(reduceDraft(typed(3), key(k))).toEqual(unknown);
		expect(reduceDraft(unknown, key(k))).toEqual(unknown);
	});
});

describe("reduceDraft — non-key events", () => {
	it.each<[string, ComposerDraft, DraftEvent, ComposerDraft]>([
		["userPaste from clean goes to unknown", clean, { type: "userPaste" }, unknown],
		["userPaste from typed goes to unknown", typed(2), { type: "userPaste" }, unknown],
		["userPaste from unknown stays unknown", unknown, { type: "userPaste" }, unknown],
		["seamPaste from clean goes to unknown", clean, { type: "seamPaste" }, unknown],
		["seamPaste from typed goes to unknown", typed(2), { type: "seamPaste" }, unknown],
		["seamSubmit from clean stays clean", clean, { type: "seamSubmit" }, clean],
		["seamSubmit from typed clears", typed(3), { type: "seamSubmit" }, clean],
		["seamSubmit from unknown clears", unknown, { type: "seamSubmit" }, clean],
		["respawn from clean stays clean", clean, { type: "respawn" }, clean],
		["respawn from typed clears", typed(5), { type: "respawn" }, clean],
		["respawn from unknown clears", unknown, { type: "respawn" }, clean],
	])("%s", (_desc, prev, ev, expected) => {
		expect(reduceDraft(prev, ev)).toEqual(expected);
	});
});

describe("reduceDraft — sequences", () => {
	it('type "ab" then two backspaces returns to clean', () => {
		const afterA = reduceDraft(clean, key("a"));
		expect(afterA).toEqual(typed(1));
		const afterB = reduceDraft(afterA, key("b"));
		expect(afterB).toEqual(typed(2));
		const afterBs1 = reduceDraft(afterB, key("\x7f"));
		expect(afterBs1).toEqual(typed(1));
		const afterBs2 = reduceDraft(afterBs1, key("\x7f"));
		expect(afterBs2).toEqual(clean);
		expect(draftClearedBy(afterBs1, key("\x7f"), afterBs2)).toBe(true);
	});

	it("type then Enter reaches clean, but is not reported as a draft-cleared trigger", () => {
		const afterA = reduceDraft(clean, key("a"));
		const ev = key("\r");
		const afterEnter = reduceDraft(afterA, ev);
		expect(afterEnter).toEqual(clean);
		expect(draftClearedBy(afterA, ev, afterEnter)).toBe(false);
	});

	it("type then Ctrl+C reaches clean and IS reported as a draft-cleared trigger", () => {
		const afterA = reduceDraft(clean, key("a"));
		const ev = key("\x03");
		const afterCtrlC = reduceDraft(afterA, ev);
		expect(afterCtrlC).toEqual(clean);
		expect(draftClearedBy(afterA, ev, afterCtrlC)).toBe(true);
	});

	it("Up on a clean composer goes to unknown", () => {
		expect(reduceDraft(clean, key("\x1b[A"))).toEqual(unknown);
	});

	it("type then Left goes to unknown, and a later backspace stays unknown", () => {
		const afterA = reduceDraft(clean, key("a"));
		const afterLeft = reduceDraft(afterA, key("\x1b[D"));
		expect(afterLeft).toEqual(unknown);
		const afterBs = reduceDraft(afterLeft, key("\x7f"));
		expect(afterBs).toEqual(unknown);
	});

	it("seamPaste then seamSubmit reaches clean, but is not reported as a draft-cleared trigger", () => {
		const afterPaste = reduceDraft(clean, { type: "seamPaste" });
		expect(afterPaste).toEqual(unknown);
		const ev: DraftEvent = { type: "seamSubmit" };
		const afterSubmit = reduceDraft(afterPaste, ev);
		expect(afterSubmit).toEqual(clean);
		expect(draftClearedBy(afterPaste, ev, afterSubmit)).toBe(false);
	});

	it("respawn from typed reaches clean, but is not reported as a draft-cleared trigger", () => {
		const t = typed(4);
		const ev: DraftEvent = { type: "respawn" };
		const afterRespawn = reduceDraft(t, ev);
		expect(afterRespawn).toEqual(clean);
		expect(draftClearedBy(t, ev, afterRespawn)).toBe(false);
	});
});

describe("draftClearedBy — additional edge cases", () => {
	it("is false when prev was already clean", () => {
		expect(draftClearedBy(clean, key("\r"), clean)).toBe(false);
	});

	it("is false when next isn't clean", () => {
		expect(draftClearedBy(typed(2), key("\x1b[D"), unknown)).toBe(false);
	});
});

describe("checkDraft", () => {
	it("allows a clean composer", () => {
		expect(checkDraft(clean)).toBeNull();
	});

	it("holds on typed", () => {
		expect(checkDraft(typed(3))).toEqual({
			ok: false,
			reason:
				"the composer holds unsent text — holding automatic delivery until it's submitted or cleared",
		});
	});

	it("holds on unknown, with the same reason as typed", () => {
		expect(checkDraft(unknown)).toEqual(checkDraft(typed(1)));
	});
});
