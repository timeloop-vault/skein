import { describe, expect, it } from "vitest";
import { relativeAge, subtitleFor } from "./subtitle.ts";

const now = 1_000_000_000_000;

describe("subtitleFor", () => {
	it("shows the recap verbatim with its age", () => {
		const s = subtitleFor({ content: "did stuff", timestampMs: now - 5 * 60_000 }, 3, now, now);
		expect(s).toEqual({ glyph: "AT", text: "did stuff", age: "5m ago", empty: false });
	});

	it("is neutral, not idle, when the feed has rows but no recap", () => {
		const s = subtitleFor(null, 2, now - 2 * 3_600_000, now);
		expect(s).toEqual({ glyph: "AT", text: "No away recap yet", age: "2h ago", empty: true });
		expect(s.text).not.toMatch(/No agent/);
	});

	it("omits the age when there is no last action time", () => {
		expect(subtitleFor(null, 1, 0, now).age).toBe("");
	});

	it("is idle when the feed is empty", () => {
		expect(subtitleFor(null, 0, 0, now)).toEqual({
			glyph: "IDLE",
			text: "No agent has worked here yet",
			age: "",
			empty: true,
		});
	});
});

describe("relativeAge", () => {
	it("buckets seconds, minutes, hours, days", () => {
		expect(relativeAge(now - 30_000, now)).toBe("30s ago");
		expect(relativeAge(now - 10 * 60_000, now)).toBe("10m ago");
		expect(relativeAge(now - 3 * 3_600_000, now)).toBe("3h ago");
		expect(relativeAge(now - 2 * 86_400_000, now)).toBe("2d ago");
		expect(relativeAge(0, now)).toBe("");
	});
});
