import { describe, expect, it } from "vitest";
import type { ClipboardKeyLike, ClipboardPlatform } from "./terminalClipboard.ts";
import { decideClipboardAction, emptySelectionHint } from "./terminalClipboard.ts";

const key = (over: Partial<ClipboardKeyLike> = {}): ClipboardKeyLike => ({
	type: "keydown",
	code: "KeyC",
	ctrlKey: false,
	shiftKey: false,
	altKey: false,
	metaKey: false,
	...over,
});

// `hasSelection` only changes the verdict for Windows' smart plain-Ctrl+C
// (see terminalClipboard.ts); default it to `false` here so every other
// test can omit an argument that's irrelevant to what it's checking.
const decide = (
	e: ClipboardKeyLike,
	platform: ClipboardPlatform,
	hasSelection = false,
): ReturnType<typeof decideClipboardAction> => decideClipboardAction(e, platform, hasSelection);

describe("decideClipboardAction", () => {
	describe("mac", () => {
		const p: ClipboardPlatform = "mac";
		it("⌘C copies", () => {
			expect(decide(key({ code: "KeyC", metaKey: true }), p)).toBe("copy");
		});
		it("plain Ctrl+C is not a copy (stays SIGINT), even with a selection", () => {
			expect(decide(key({ code: "KeyC", ctrlKey: true }), p, true)).toBeNull();
		});
		it("Ctrl+Shift+C is not the mac combo", () => {
			expect(decide(key({ code: "KeyC", ctrlKey: true, shiftKey: true }), p)).toBeNull();
		});
		it("⌘V is never intercepted — native paste handles it", () => {
			expect(decide(key({ code: "KeyV", metaKey: true }), p)).toBeNull();
		});
		it("Ctrl+V is not intercepted on mac", () => {
			expect(decide(key({ code: "KeyV", ctrlKey: true }), p)).toBeNull();
		});
	});

	describe("windows", () => {
		const p: ClipboardPlatform = "windows";
		it("Ctrl+Shift+C always copies, regardless of selection", () => {
			expect(decide(key({ code: "KeyC", ctrlKey: true, shiftKey: true }), p, false)).toBe("copy");
			expect(decide(key({ code: "KeyC", ctrlKey: true, shiftKey: true }), p, true)).toBe("copy");
		});
		it("smart plain Ctrl+C copies when there's a live selection", () => {
			expect(decide(key({ code: "KeyC", ctrlKey: true }), p, true)).toBe("copy");
		});
		it("smart plain Ctrl+C stays SIGINT with no selection", () => {
			expect(decide(key({ code: "KeyC", ctrlKey: true }), p, false)).toBeNull();
		});
		it("⌘C is not the windows combo", () => {
			expect(decide(key({ code: "KeyC", metaKey: true }), p, true)).toBeNull();
		});
		it("Ctrl+Alt+C (AltGr) is not intercepted, even with a selection", () => {
			expect(decide(key({ code: "KeyC", ctrlKey: true, altKey: true }), p, true)).toBeNull();
		});
		it("plain Ctrl+V falls back to the clipboard manager", () => {
			expect(decide(key({ code: "KeyV", ctrlKey: true }), p)).toBe("paste");
		});
		it("Ctrl+Shift+V also falls back", () => {
			expect(decide(key({ code: "KeyV", ctrlKey: true, shiftKey: true }), p)).toBe("paste");
		});
		it("Ctrl+Alt+V (AltGr) is not intercepted", () => {
			expect(decide(key({ code: "KeyV", ctrlKey: true, altKey: true }), p)).toBeNull();
		});
	});

	describe("linux", () => {
		const p: ClipboardPlatform = "linux";
		it("Ctrl+Shift+C copies", () => {
			expect(decide(key({ code: "KeyC", ctrlKey: true, shiftKey: true }), p)).toBe("copy");
		});
		it("plain Ctrl+C stays SIGINT unconditionally — no smart-Ctrl+C here, even with a selection", () => {
			expect(decide(key({ code: "KeyC", ctrlKey: true }), p, true)).toBeNull();
		});
		it("plain Ctrl+V stays native/PTY (\\x16 — Claude Code image paste)", () => {
			expect(decide(key({ code: "KeyV", ctrlKey: true }), p)).toBeNull();
		});
		it("Ctrl+Shift+V falls back to the clipboard manager", () => {
			expect(decide(key({ code: "KeyV", ctrlKey: true, shiftKey: true }), p)).toBe("paste");
		});
		it("Ctrl+Alt+V (AltGr) is not intercepted", () => {
			expect(decide(key({ code: "KeyV", ctrlKey: true, altKey: true }), p)).toBeNull();
		});
	});

	describe("cross-platform edges", () => {
		it("keyup is never a shortcut", () => {
			expect(
				decide(key({ type: "keyup", code: "KeyC", ctrlKey: true, shiftKey: true }), "windows"),
			).toBeNull();
			expect(decide(key({ type: "keyup", code: "KeyV", metaKey: true }), "mac")).toBeNull();
		});
		it("unrelated keys are never a shortcut", () => {
			expect(decide(key({ code: "KeyA", ctrlKey: true }), "windows", true)).toBeNull();
		});
	});
});

describe("emptySelectionHint", () => {
	it("mentions Option+drag on mac when mouse tracking is on", () => {
		expect(emptySelectionHint("mac", true)).toBe(
			"Nothing selected — Option+drag to select over the agent's UI",
		);
	});
	it("mentions Shift+drag on windows/linux when mouse tracking is on", () => {
		expect(emptySelectionHint("windows", true)).toBe(
			"Nothing selected — Shift+drag to select over the agent's UI",
		);
		expect(emptySelectionHint("linux", true)).toBe(
			"Nothing selected — Shift+drag to select over the agent's UI",
		);
	});
	it("is plain when mouse tracking is off — no forced-selection chord needed", () => {
		expect(emptySelectionHint("mac", false)).toBe("Nothing selected");
		expect(emptySelectionHint("windows", false)).toBe("Nothing selected");
	});
});
