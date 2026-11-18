// The link-underline rule on small hand-made pictures (#793).
import { describe, expect, it } from "vitest";
import { compare } from "./compare.ts";
import { excuseLinkUnderline, withoutUnderline } from "./linkUnderline.ts";
import { DEFAULT_STYLE, type Picture, type Row } from "./picture.ts";

const U = "fg=def bg=def underline";
const row = (style: string, link: string): Row => ({
	wrapped: false,
	runs: [{ col: 0, end: 2, style, text: "ab", link }],
});
function pic(r: Row): Picture {
	return {
		rows: 1,
		cols: 4,
		altScreen: false,
		cursor: { row: 0, col: 0, visible: true },
		modes: {
			appCursorKeys: false,
			appKeypad: false,
			bracketedPaste: false,
			focusReporting: false,
			mouseTracking: "off",
			syncPending: false,
		},
		history: [],
		screen: [r],
	};
}
const styleDiff = (a: Row, b: Row) =>
	compare(pic(a), pic(b)).diffs.filter((d) => d.category === "screenStyles").length;

describe("link underline rule", () => {
	it("strips only the underline flag", () => {
		expect(withoutUnderline(U)).toBe(DEFAULT_STYLE);
		expect(withoutUnderline("fg=def bg=def bold,underline")).toBe("fg=def bg=def bold");
		expect(withoutUnderline(DEFAULT_STYLE)).toBeUndefined();
	});

	it("excuses xterm's underline on a link both sides carry", () => {
		const a = row(U, "http://x");
		const b = row(DEFAULT_STYLE, "http://x");
		expect(excuseLinkUnderline([a], [b]).excused).toBe(2);
		expect(styleDiff(a, b)).toBe(0);
		expect(compare(pic(a), pic(b)).excused).toBe(2);
	});

	it("does not excuse the reverse: engine underlined, xterm not", () => {
		expect(styleDiff(row(DEFAULT_STYLE, "http://x"), row(U, "http://x"))).toBe(1);
	});

	it("does not excuse a bold difference on a link cell", () => {
		const a = row("fg=def bg=def bold,underline", "http://x");
		expect(styleDiff(a, row(DEFAULT_STYLE, "http://x"))).toBe(1);
		expect(styleDiff(row(U, "http://x"), row("fg=def bg=def bold", "http://x"))).toBe(1);
	});

	it("does not excuse a link on one side only, or two different links", () => {
		expect(styleDiff(row(U, "http://x"), row(DEFAULT_STYLE, ""))).toBe(1);
		expect(styleDiff(row(U, "http://x"), row(DEFAULT_STYLE, "http://y"))).toBe(1);
		expect(styleDiff(row(U, ""), row(DEFAULT_STYLE, ""))).toBe(1);
	});
});
