import { describe, expect, it } from "vitest";
import { relativeTime } from "./relativeTime.ts";

describe("relativeTime", () => {
	const now = 10_000_000_000;
	it("buckets seconds, minutes, hours, days", () => {
		expect(relativeTime(now - 5_000, now)).toBe("just now");
		expect(relativeTime(now - 50_000, now)).toBe("1m ago");
		expect(relativeTime(now - 3 * 60_000, now)).toBe("3m ago");
		expect(relativeTime(now - 2 * 3_600_000, now)).toBe("2h ago");
		expect(relativeTime(now - 3 * 86_400_000, now)).toBe("3d ago");
	});
	it("treats a future timestamp as just now", () => {
		expect(relativeTime(now + 60_000, now)).toBe("just now");
	});
});
