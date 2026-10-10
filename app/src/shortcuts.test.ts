import { describe, expect, it } from "vitest";
import { hints, isAppShortcut, matchShortcut } from "./shortcuts.ts";

// Node env: `isMac` is false, so the primary modifier is Alt.
const key = (code: string, init: Partial<KeyboardEvent> = {}) =>
	({
		code,
		altKey: true,
		metaKey: false,
		shiftKey: false,
		ctrlKey: false,
		...init,
	}) as KeyboardEvent;

describe("todo shortcuts (#335)", () => {
	it("Mod+T adds a room todo, Mod+Shift+T a global one", () => {
		expect(matchShortcut(key("KeyT"))?.action).toBe("addRoomTodo");
		expect(matchShortcut(key("KeyT", { shiftKey: true }))?.action).toBe("addGlobalTodo");
	});
	it("a bare T is not an app shortcut", () => {
		expect(matchShortcut(key("KeyT", { altKey: false }))).toBeNull();
	});
});

describe("close shortcuts (#307)", () => {
	it("Mod+W closes the room, Mod+Shift+W the active harness", () => {
		expect(matchShortcut(key("KeyW"))?.action).toBe("closeRoom");
		expect(matchShortcut(key("KeyW", { shiftKey: true }))?.action).toBe("closeHarness");
	});
	it("Mod+Shift+W is swallowed from the terminal as an app shortcut", () => {
		expect(isAppShortcut(key("KeyW", { shiftKey: true }))).toBe(true);
	});
	it("has a hint", () => {
		expect(hints.closeHarness).toContain("⇧ W");
	});
});
