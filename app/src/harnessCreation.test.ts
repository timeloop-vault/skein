import { describe, expect, it } from "vitest";
import {
	buildHarness,
	buildRoom,
	claimedSessionIdsOf,
	harnessDisplayName,
	resolveAgentName,
	withCapturedSessionId,
	withReplacedSessionId,
} from "./harnessCreation.ts";
import type { Room } from "./types.ts";

const room = (sessionId?: string): Room =>
	buildRoom({
		roomId: "s1",
		harness: buildHarness({
			id: "h1",
			kind: "claude",
			name: "main",
			cwd: "/x",
			cmd: ["claude"],
			sessionId,
			agentName: undefined,
		}),
		cwd: "/x",
		task: "t",
		folderName: "x",
		activate: true,
	});

describe("harnessCreation", () => {
	it("builds a pty harness as running/live, with cmd and optional fields only when set", () => {
		const h = buildHarness({
			id: "h",
			kind: "claude",
			name: "main",
			cwd: "/x",
			cmd: ["claude"],
			sessionId: "sid",
			agentName: "a",
			createdBy: { roomId: "r", harnessId: "q" } as never,
		});
		expect(h).toMatchObject({
			status: "running",
			live: true,
			cmd: ["claude"],
			sessionId: "sid",
			agent: "a",
		});
		expect(h.createdBy).toBeDefined();
		const bare = buildHarness({
			id: "h",
			kind: "files",
			name: "files-1",
			cwd: "/x",
			cmd: undefined,
			sessionId: undefined,
			agentName: undefined,
		});
		expect(bare).toMatchObject({ status: "idle", model: "" });
		for (const k of ["live", "cmd", "sessionId", "agent", "createdBy"])
			expect(k in bare).toBe(false);
	});

	it("names harnesses by kind label, files/design by kind", () => {
		expect(harnessDisplayName("files", 0)).toBe("files-1");
		expect(harnessDisplayName("design", 2)).toBe("design-3");
	});

	it("drops a blank or unsupported agent", () => {
		expect(resolveAgentName("claude", "  ")).toBeUndefined();
		expect(resolveAgentName("byoh", "x")).toBeUndefined();
		expect(resolveAgentName("claude", "x")).toBe("x");
	});

	it("marks a background room as attention and leaves it off when activated", () => {
		expect(room().attention).toBeUndefined();
		const bg = buildRoom({
			roomId: "s",
			harness: buildHarness({
				id: "h",
				kind: "claude",
				name: "main",
				cwd: "/x",
				cmd: undefined,
				sessionId: undefined,
				agentName: undefined,
			}),
			cwd: "/x",
			task: "t",
			branch: "b",
			folderName: "x",
			activate: false,
		});
		expect(bg).toMatchObject({ attention: true, branch: "b", repo: "x" });
	});

	it("captures a session id first-writer-wins, replaces authoritatively", () => {
		const unset = [room()];
		const captured = withCapturedSessionId(unset, "s1", "h1", "a");
		expect(captured[0]?.harnesses[0]?.sessionId).toBe("a");
		expect(withCapturedSessionId(captured, "s1", "h1", "b")[0]?.harnesses[0]?.sessionId).toBe("a");
		expect(withReplacedSessionId(captured, "s1", "h1", "b")[0]?.harnesses[0]?.sessionId).toBe("b");
		expect(withReplacedSessionId(captured, "nope", "h1", "b")[0]?.harnesses[0]?.sessionId).toBe(
			"a",
		);
	});

	it("collects claimed session ids", () => {
		expect([...claimedSessionIdsOf([room("a"), room()])]).toEqual(["a"]);
	});
});
