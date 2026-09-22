import { describe, expect, it } from "vitest";
import { followedOpencodeSession, followedSession } from "./sessionTracking.ts";

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

	it.each(["startup", "compact", null, undefined])(
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

	it("fork with a new session id returns that id, source fork", () => {
		expect(followedSession("old-sid", { sessionId: "new-sid", source: "fork" })).toEqual({
			sessionId: "new-sid",
			source: "fork",
		});
	});

	it("fork with the same session id returns null", () => {
		expect(followedSession("same-sid", { sessionId: "same-sid", source: "fork" })).toBeNull();
	});

	it("fork with a missing session id returns null", () => {
		expect(followedSession("old-sid", { source: "fork" })).toBeNull();
	});

	it("current undefined + fork returns the new id", () => {
		expect(followedSession(undefined, { sessionId: "new-sid", source: "fork" })).toEqual({
			sessionId: "new-sid",
			source: "fork",
		});
	});
});

describe("followedOpencodeSession (#116)", () => {
	it("session_created for a child (non-null parentId) is never followed", () => {
		expect(
			followedOpencodeSession("root-sid", {
				kind: "session_created",
				sessionId: "child-sid",
				parentId: "root-sid",
			}),
		).toBeNull();
	});

	it("session_created for a child is ignored even with no current session yet", () => {
		expect(
			followedOpencodeSession(undefined, {
				kind: "session_created",
				sessionId: "child-sid",
				parentId: "root-sid",
			}),
		).toBeNull();
	});

	it("session_created root with no current session returns null (initial capture owns this)", () => {
		expect(
			followedOpencodeSession(undefined, {
				kind: "session_created",
				sessionId: "new-sid",
				parentId: null,
			}),
		).toBeNull();
	});

	it("session_created root with a different id than current returns source new", () => {
		expect(
			followedOpencodeSession("old-sid", {
				kind: "session_created",
				sessionId: "new-sid",
				parentId: null,
			}),
		).toEqual({ sessionId: "new-sid", source: "new" });
	});

	it("session_created root with the same id as current returns null", () => {
		expect(
			followedOpencodeSession("same-sid", {
				kind: "session_created",
				sessionId: "same-sid",
				parentId: null,
			}),
		).toBeNull();
	});

	it("root_session_prompted with no current session returns null (treated as capture, not follow)", () => {
		expect(
			followedOpencodeSession(undefined, {
				kind: "root_session_prompted",
				sessionId: "other-sid",
			}),
		).toBeNull();
	});

	it("root_session_prompted with a different id than current returns source switch", () => {
		expect(
			followedOpencodeSession("old-sid", {
				kind: "root_session_prompted",
				sessionId: "other-sid",
			}),
		).toEqual({ sessionId: "other-sid", source: "switch" });
	});

	it("root_session_prompted with the same id as current returns null", () => {
		expect(
			followedOpencodeSession("same-sid", {
				kind: "root_session_prompted",
				sessionId: "same-sid",
			}),
		).toBeNull();
	});
});
