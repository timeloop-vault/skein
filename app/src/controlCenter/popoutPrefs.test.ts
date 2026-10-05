import { describe, expect, it } from "vitest";
import { DEFAULT_CC_POPOUT, parseCcPopout } from "./popoutPrefs.ts";

describe("parseCcPopout", () => {
	it("defaults for missing, corrupt and non-object input", () => {
		for (const raw of [null, "{", "42", "null"]) {
			expect(parseCcPopout(raw)).toEqual(DEFAULT_CC_POPOUT);
		}
	});

	it("reads a full value", () => {
		const g = { x: -10, y: 20, width: 800, height: 500 };
		expect(parseCcPopout(JSON.stringify({ open: true, alwaysOnTop: true, geometry: g }))).toEqual({
			open: true,
			alwaysOnTop: true,
			geometry: g,
		});
	});

	it("only accepts literal true and drops bad geometry", () => {
		const p = parseCcPopout(
			JSON.stringify({
				open: "yes",
				alwaysOnTop: 1,
				geometry: { x: 0, y: 0, width: 5, height: 500 },
			}),
		);
		expect(p).toEqual({ open: false, alwaysOnTop: false });
		expect("geometry" in p).toBe(false);
	});
});
