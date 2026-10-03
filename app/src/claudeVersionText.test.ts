import { describe, expect, it } from "vitest";
import { badgeTitle, restartMenuLabel, updateSegmentText } from "./claudeVersionText.ts";

const n = { running: "2.1.280", installed: "2.1.288" };

describe("claudeVersionText", () => {
	it("words the popover segment", () => {
		expect(updateSegmentText(n, null)).toBe("Claude Code 2.1.280 → 2.1.288 available");
		expect(updateSegmentText(n, "a turn is running")).toBe(
			"Claude Code 2.1.280 → 2.1.288 available · not restarted: a turn is running",
		);
	});
	it("words the badge tooltip", () => {
		expect(badgeTitle(n, null)).toBe("Claude Code 2.1.280 → 2.1.288 available — click to restart");
		expect(badgeTitle(n, "busy")).toBe(
			"Claude Code 2.1.280 → 2.1.288 available — click to restart\nNot restarted: busy",
		);
	});
	it("words the menu item", () => {
		expect(restartMenuLabel(n)).toBe("Restart to update (2.1.280 → 2.1.288)");
		expect(restartMenuLabel(null)).toBe("Restart harness");
	});
});
