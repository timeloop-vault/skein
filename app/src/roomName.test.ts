import { describe, expect, it } from "vitest";
import { commitRoomName, defaultRoomName } from "./roomName.ts";

describe("defaultRoomName", () => {
	it("takes the trailing component of a Windows path", () => {
		expect(defaultRoomName("D:\\code\\skein")).toBe("skein");
	});

	it("takes the trailing component of a POSIX path with a trailing slash", () => {
		expect(defaultRoomName("/a/skein-wt/skein-241/")).toBe("skein-241");
	});

	it("takes the trailing component of a Windows path with a trailing backslash", () => {
		expect(defaultRoomName("D:\\code\\skein\\")).toBe("skein");
	});

	it("handles mixed separators", () => {
		expect(defaultRoomName("/a/skein-wt\\skein-241")).toBe("skein-241");
	});

	it("falls back to the input for a root-ish path with no trailing component", () => {
		expect(defaultRoomName("/")).toBe("/");
	});

	it("strips a trailing backslash before taking the drive letter itself", () => {
		// Only one separator, so what's left after stripping it isn't empty —
		// "C:" is itself a (degenerate) trailing component, not a fallback.
		expect(defaultRoomName("C:\\")).toBe("C:");
	});
});

describe("commitRoomName", () => {
	it("uses the trimmed input", () => {
		expect(commitRoomName("old", "  new name  ")).toBe("new name");
	});

	it("falls back to the previous name when the input is empty", () => {
		expect(commitRoomName("old", "")).toBe("old");
	});

	it("falls back to the previous name when the input is whitespace-only", () => {
		expect(commitRoomName("old", "   ")).toBe("old");
	});

	it("keeps a name unchanged when it round-trips", () => {
		expect(commitRoomName("old", "old")).toBe("old");
	});
});
