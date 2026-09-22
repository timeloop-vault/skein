import { describe, expect, it } from "vitest";
import { followedSession } from "./sessionTracking.ts";

describe("followedSession (#116)", () => {
	it("clear with a new session id returns that id, source clear", () => {
		expect(followedSession("old-sid", { sessionId: "new-sid", source: "clear" })).toEqual({
			sessionId: "new-sid",
			source: "clear",
		});
	});

	it("clear with the same session id returns null", () => {
		expect(followedSession("same-sid", { sessionId: "same-sid", source: "clear" })).toBeNull();
	});

	it("clear with a missing session id returns null", () => {
		expect(followedSession("old-sid", { source: "clear" })).toBeNull();
	});

	it("clear with an empty session id returns null", () => {
		expect(followedSession("old-sid", { sessionId: "", source: "clear" })).toBeNull();
	});

	it("clear with a null session id returns null", () => {
		expect(followedSession("old-sid", { sessionId: null, source: "clear" })).toBeNull();
	});

	it("resume with a new session id returns that id, source resume", () => {
		expect(followedSession("old-sid", { sessionId: "new-sid", source: "resume" })).toEqual({
			sessionId: "new-sid",
			source: "resume",
		});
	});

	it("resume with the same session id returns null (Skein's own `claude --resume <sid>` boot resume)", () => {
		expect(followedSession("same-sid", { sessionId: "same-sid", source: "resume" })).toBeNull();
	});

	it("resume with a missing session id returns null", () => {
		expect(followedSession("old-sid", { source: "resume" })).toBeNull();
	});

	it.each(["startup", "compact", "fork", null, undefined])(
		"source %s with a different session id returns null",
		(source) => {
			expect(
				followedSession("old-sid", { sessionId: "new-sid", source: source as string | null }),
			).toBeNull();
		},
	);

	it("current undefined + clear returns the new id", () => {
		expect(followedSession(undefined, { sessionId: "new-sid", source: "clear" })).toEqual({
			sessionId: "new-sid",
			source: "clear",
		});
	});

	it("current undefined + resume returns the new id", () => {
		expect(followedSession(undefined, { sessionId: "new-sid", source: "resume" })).toEqual({
			sessionId: "new-sid",
			source: "resume",
		});
	});
});
