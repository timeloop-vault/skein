import { afterEach, describe, expect, it, vi } from "vitest";

// shortcuts.ts reads `navigator.platform` at import time, so each platform
// gets a fresh module instance under a stubbed navigator. That makes the
// suite independent of the OS it runs on.
const PLATFORMS = [
	{
		name: "mac",
		platform: "MacIntel",
		isMac: true,
		mod: { metaKey: true },
		wrongMod: { altKey: true },
		label: "⌘",
	},
	{
		name: "windows",
		platform: "Win32",
		isMac: false,
		mod: { altKey: true },
		wrongMod: { metaKey: true },
		label: "Alt",
	},
	{
		name: "linux",
		platform: "Linux x86_64",
		isMac: false,
		mod: { altKey: true },
		wrongMod: { metaKey: true },
		label: "Alt",
	},
] as const;

const load = async (platform: string) => {
	vi.stubGlobal("navigator", { platform });
	vi.resetModules();
	return import("./shortcuts.ts");
};

const ev = (code: string, init: Partial<KeyboardEvent> = {}) =>
	({
		code,
		altKey: false,
		metaKey: false,
		shiftKey: false,
		ctrlKey: false,
		...init,
	}) as KeyboardEvent;

afterEach(() => {
	vi.unstubAllGlobals();
});

describe.each(PLATFORMS)("shortcuts on $name", (p) => {
	it("detects the platform from the stub", async () => {
		const m = await load(p.platform);
		expect(m.isMac).toBe(p.isMac);
		expect(m.modLabel).toBe(p.label);
	});

	describe("todo shortcuts (#335)", () => {
		it("Mod+T adds a room todo, Mod+Shift+T a global one", async () => {
			const { matchShortcut } = await load(p.platform);
			expect(matchShortcut(ev("KeyT", p.mod))?.action).toBe("addRoomTodo");
			expect(matchShortcut(ev("KeyT", { ...p.mod, shiftKey: true }))?.action).toBe("addGlobalTodo");
		});
		it("a bare T, or the other platform's modifier, is not an app shortcut", async () => {
			const { matchShortcut } = await load(p.platform);
			expect(matchShortcut(ev("KeyT"))).toBeNull();
			expect(matchShortcut(ev("KeyT", p.wrongMod))).toBeNull();
		});
	});

	describe("close shortcuts (#307)", () => {
		it("Mod+W closes the room, Mod+Shift+W the active harness", async () => {
			const { matchShortcut } = await load(p.platform);
			expect(matchShortcut(ev("KeyW", p.mod))?.action).toBe("closeRoom");
			expect(matchShortcut(ev("KeyW", { ...p.mod, shiftKey: true }))?.action).toBe("closeHarness");
		});
		it("Mod+Shift+W is swallowed from the terminal as an app shortcut", async () => {
			const { isAppShortcut } = await load(p.platform);
			expect(isAppShortcut(ev("KeyW", { ...p.mod, shiftKey: true }))).toBe(true);
		});
		it("has a hint", async () => {
			const { hints } = await load(p.platform);
			expect(hints.closeHarness).toBe(`${p.label} ⇧ W`);
		});
	});
});
