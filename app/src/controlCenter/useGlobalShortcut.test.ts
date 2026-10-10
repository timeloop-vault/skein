import { describe, expect, it } from "vitest";
import { GLOBAL_CC_SHORTCUT_DEFAULT, GLOBAL_CC_SHORTCUT_KEY } from "../prefs.ts";
import { GLOBAL_CONTROL_CENTER_ACCELERATOR, GLOBAL_CONTROL_CENTER_LABEL } from "../shortcuts.ts";
import { createSerial, isPress } from "./useGlobalShortcut.ts";

describe("isPress", () => {
	it("accepts Pressed only", () => {
		expect(isPress({ state: "Pressed" })).toBe(true);
		expect(isPress({ state: "Released" })).toBe(false);
	});
});

describe("createSerial", () => {
	it("runs tasks in order even when an earlier one is slower or fails", async () => {
		const serial = createSerial();
		const log: string[] = [];
		const a = serial(async () => {
			await new Promise((r) => setTimeout(r, 20));
			log.push("a");
			throw new Error("boom");
		});
		const b = serial(async () => {
			log.push("b");
		});
		await expect(a).rejects.toThrow("boom");
		await b;
		expect(log).toEqual(["a", "b"]);
	});
});

describe("global shortcut definition", () => {
	it("is Ctrl/Cmd+Shift+0", () => {
		expect(GLOBAL_CONTROL_CENTER_ACCELERATOR).toBe("CommandOrControl+Shift+0");
		expect(GLOBAL_CONTROL_CENTER_LABEL).toMatch(/⇧ 0$/);
	});
	it("is on by default (absent key = enabled)", () => {
		expect(GLOBAL_CC_SHORTCUT_DEFAULT).toBe(true);
		expect(GLOBAL_CC_SHORTCUT_KEY).toBe("globalCcShortcut");
	});
});
