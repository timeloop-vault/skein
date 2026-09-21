import { describe, expect, it } from "vitest";
import { clearedSessionId } from "./sessionTracking.ts";

describe("clearedSessionId (#116)", () => {
	it("clear with a new session id returns that id", () => {
		expect(clearedSessionId("old-sid", { sessionId: "new-sid", source: "clear" })).toBe("new-sid");
	});

	it("clear with the same session id returns null", () => {
		expect(clearedSessionId("same-sid", { sessionId: "same-sid", source: "clear" })).toBeNull();
	});

	it("clear with a missing session id returns null", () => {
		expect(clearedSessionId("old-sid", { source: "clear" })).toBeNull();
	});

	it("clear with an empty session id returns null", () => {
		expect(clearedSessionId("old-sid", { sessionId: "", source: "clear" })).toBeNull();
	});

	it("clear with a null session id returns null", () => {
		expect(clearedSessionId("old-sid", { sessionId: null, source: "clear" })).toBeNull();
	});

	it.each(["startup", "compact", "resume", "fork", null, undefined])(
		"source %s with a different session id returns null",
		(source) => {
			expect(
				clearedSessionId("old-sid", { sessionId: "new-sid", source: source as string | null }),
			).toBeNull();
		},
	);

	it("current undefined + clear returns the new id", () => {
		expect(clearedSessionId(undefined, { sessionId: "new-sid", source: "clear" })).toBe("new-sid");
	});
});
