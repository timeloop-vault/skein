import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { harnessActivity } from "./harnessActivity.ts";
import { SCREEN_SETTLE_MS, harnessInput, sendPrompt } from "./harnessInput.ts";
import type { HarnessInputTarget } from "./harnessInput.ts";
import type { ScreenCell, ScreenSnapshot } from "./promptScreen.ts";

// #413 — the screen check that releases an `unknown` composer latch.
beforeAll(() => {
	vi.useFakeTimers();
});
afterAll(() => {
	vi.useRealTimers();
});

const nextId = (() => {
	let n = 0;
	return () => `scr_${++n}`;
})();

const COLS = 20;
const rowOf = (text: string): ScreenCell[] =>
	Array.from({ length: COLS }, (_, x) => ({ ch: [...text][x] ?? " ", dim: false }));
const RULE = "─".repeat(COLS);

/// A Claude composer between two rules; the input row is row 1.
const claudeScreen = (input: string, cursorX: number): ScreenSnapshot => ({
	rows: [rowOf(RULE), rowOf(input), rowOf(RULE)],
	cursorX,
	cursorY: 1,
});
const EMPTY = claudeScreen("❯ ", 2);
const TEXT = claudeScreen("❯ hi", 4);

const setup = (screen?: () => ScreenSnapshot | null) => {
	const id = nextId();
	const target: HarnessInputTarget = {
		paste: vi.fn(),
		bracketedPaste: () => false,
		submit: vi.fn(),
		kind: "claude",
		...(screen ? { screen } : {}),
	};
	harnessInput.register(id, target);
	return { id, target };
};
const key = (id: string, k: string) => harnessInput.noteDraftEvent(id, { type: "key", key: k });
/// Type "a", then a cursor move: the draft is `unknown`.
const makeUnknown = (id: string) => {
	key(id, "a");
	key(id, "\x1b[D");
	expect(harnessInput.draft(id).kind).toBe("unknown");
};

describe("harnessInput.checkScreen", () => {
	it("releases unknown to clean after the settle window and fires draftCleared", () => {
		const { id } = setup(() => EMPTY);
		const cleared = vi.fn();
		const unsub = harnessInput.subscribeDraftCleared(cleared);
		makeUnknown(id);
		vi.advanceTimersByTime(SCREEN_SETTLE_MS - 1);
		expect(harnessInput.checkScreen(id)).toBe(false);
		expect(harnessInput.draft(id).kind).toBe("unknown");
		vi.advanceTimersByTime(1);
		// The debounced timer fires at the boundary.
		expect(harnessInput.draft(id).kind).toBe("clean");
		expect(cleared).toHaveBeenCalledWith(id);
		expect(harnessInput.draftCause(id)).toBeNull();
		unsub();
	});

	it("stays unknown within the settle window", () => {
		const { id } = setup(() => EMPTY);
		makeUnknown(id);
		vi.advanceTimersByTime(SCREEN_SETTLE_MS - 10);
		expect(harnessInput.checkScreen(id)).toBe(false);
		expect(harnessInput.draft(id).kind).toBe("unknown");
	});

	it.each<[string, () => ScreenSnapshot | null]>([
		["text", () => TEXT],
		["an unrecognised layout", () => claudeScreen("no composer here", 3)],
		["a null snapshot", () => null],
		[
			"a throwing snapshot",
			() => {
				throw new Error("boom");
			},
		],
	])("stays unknown when the screen gives %s", (_n, screen) => {
		const { id } = setup(screen);
		makeUnknown(id);
		vi.advanceTimersByTime(SCREEN_SETTLE_MS + 100);
		expect(harnessInput.checkScreen(id)).toBe(false);
		expect(harnessInput.draft(id).kind).toBe("unknown");
	});

	it("does nothing without a screen reader", () => {
		const { id } = setup();
		makeUnknown(id);
		vi.advanceTimersByTime(SCREEN_SETTLE_MS + 100);
		expect(harnessInput.checkScreen(id)).toBe(false);
		expect(harnessInput.draft(id).kind).toBe("unknown");
	});

	it("never overrides typed", () => {
		const { id } = setup(() => EMPTY);
		key(id, "a");
		vi.advanceTimersByTime(SCREEN_SETTLE_MS + 100);
		expect(harnessInput.checkScreen(id)).toBe(false);
		expect(harnessInput.draft(id)).toEqual({ kind: "typed", chars: 1 });
	});

	it("debounces: one check after the LAST user event", () => {
		const screen = vi.fn(() => EMPTY);
		const { id } = setup(screen);
		makeUnknown(id);
		vi.advanceTimersByTime(SCREEN_SETTLE_MS - 100);
		key(id, "\x1b[C"); // resets the timer
		vi.advanceTimersByTime(SCREEN_SETTLE_MS - 100);
		expect(screen).not.toHaveBeenCalled();
		vi.advanceTimersByTime(100);
		expect(screen).toHaveBeenCalledTimes(1);
		expect(harnessInput.draft(id).kind).toBe("clean");
		vi.advanceTimersByTime(SCREEN_SETTLE_MS * 3);
		expect(screen).toHaveBeenCalledTimes(1);
	});

	it("does not treat a seam paste's unknown as the user's", () => {
		const screen = vi.fn(() => EMPTY);
		const { id } = setup(screen);
		// The seam's own term.paste echoes through onData as a userPaste first.
		harnessInput.noteDraftEvent(id, { type: "userPaste" });
		harnessInput.noteDraftEvent(id, { type: "seamPaste" });
		vi.advanceTimersByTime(SCREEN_SETTLE_MS * 3);
		expect(harnessInput.checkScreen(id)).toBe(false);
		expect(screen).not.toHaveBeenCalled();
		expect(harnessInput.draft(id).kind).toBe("unknown");
	});

	it("records the cause by event class only", () => {
		const { id } = setup(() => TEXT);
		key(id, "s3cret");
		expect(harnessInput.draftCause(id)?.event).toBe("printable");
		key(id, "\x1b[A");
		expect(harnessInput.draftCause(id)?.event).toBe("historyRecall");
	});
});

describe("sendPrompt's automatic draft recheck (#413)", () => {
	const sendable = (screen: () => ScreenSnapshot | null) => {
		const id = nextId();
		const target: HarnessInputTarget = {
			paste: vi.fn(),
			bracketedPaste: () => false,
			submit: vi.fn(),
			kind: "claude",
			screen,
		};
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		harnessActivity.adapterDelivered(id);
		harnessActivity.setWaitingFromAdapter(id, "test");
		harnessActivity.setInjected(id, true);
		harnessInput.register(id, target);
		return { id, target };
	};

	it("delivers when the settled screen reads empty", () => {
		const { id, target } = sendable(() => EMPTY);
		harnessInput.noteDraftEvent(id, { type: "userPaste" });
		const r = sendPrompt(id, "claude", "hi", { automatic: true });
		expect(r.ok).toBe(false); // still inside the settle window
		vi.advanceTimersByTime(SCREEN_SETTLE_MS);
		expect(harnessInput.draft(id).kind).toBe("clean");
		const r2 = sendPrompt(id, "claude", "hi", { automatic: true });
		expect(r2.ok).toBe(true);
		expect(target.paste).toHaveBeenCalled();
	});

	it("still holds when the screen reads text", () => {
		const { id, target } = sendable(() => TEXT);
		harnessInput.noteDraftEvent(id, { type: "userPaste" });
		vi.advanceTimersByTime(SCREEN_SETTLE_MS + 100);
		const r = sendPrompt(id, "claude", "hi", { automatic: true });
		expect(r.ok).toBe(false);
		expect(target.paste).not.toHaveBeenCalled();
	});
});
