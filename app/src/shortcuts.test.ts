import { describe, expect, it } from "vitest";
import { matchShortcut } from "./shortcuts.ts";

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
