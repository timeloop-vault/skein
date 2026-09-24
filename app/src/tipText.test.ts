import { describe, expect, it } from "vitest";
import { truncateTip } from "./tipText.ts";

describe("truncateTip", () => {
	it("returns a short body unchanged", () => {
		expect(truncateTip("one\ntwo\nthree")).toBe("one\ntwo\nthree");
	});

	it("returns a single-line body unchanged regardless of length", () => {
		const long = "x".repeat(500);
		expect(truncateTip(long)).toBe(long);
	});

	it("leaves a body at exactly the cap unchanged", () => {
		const lines = Array.from({ length: 12 }, (_, i) => `line ${i}`);
		expect(truncateTip(lines.join("\n"), 12)).toBe(lines.join("\n"));
	});

	it("cuts a body over the cap to N lines plus an ellipsis line", () => {
		const lines = Array.from({ length: 20 }, (_, i) => `line ${i}`);
		const result = truncateTip(lines.join("\n"), 12);
		expect(result).toBe(`${lines.slice(0, 12).join("\n")}\n…`);
	});

	it("respects a custom maxLines", () => {
		expect(truncateTip("a\nb\nc\nd", 2)).toBe("a\nb\n…");
	});
});
