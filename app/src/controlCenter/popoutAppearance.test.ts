import { describe, expect, it } from "vitest";
import {
	appearanceClassName,
	DEFAULT_APPEARANCE,
	isAppearanceKey,
	readAppearance,
} from "./popoutAppearance.ts";

const store = (o: Record<string, string>) => (k: string) => o[k] ?? null;

describe("readAppearance", () => {
	it("defaults when nothing is stored", () => {
		expect(readAppearance(store({}))).toEqual(DEFAULT_APPEARANCE);
	});
	it("reads the JSON-encoded prefs", () => {
		const a = readAppearance(
			store({
				"skein:theme": '"light"',
				"skein:density": '"comfy"',
				"skein:chromeFontPt": "15",
			}),
		);
		expect(a).toEqual({ theme: "light", density: "comfy", chromeFontPt: 15 });
	});
	it("ignores garbage and clamps the font size", () => {
		expect(
			readAppearance(
				store({ "skein:theme": "{", "skein:density": '"huge"', "skein:chromeFontPt": "99" }),
			),
		).toEqual({ ...DEFAULT_APPEARANCE, chromeFontPt: 20 });
	});
});

describe("helpers", () => {
	it("matches only appearance keys", () => {
		expect(isAppearanceKey("skein:theme")).toBe(true);
		expect(isAppearanceKey(null)).toBe(true);
		expect(isAppearanceKey("skein:ccPopout")).toBe(false);
	});
	it("builds the root class", () => {
		expect(appearanceClassName(DEFAULT_APPEARANCE)).toBe("cc-popout-root sk-dark density-regular");
	});
});
