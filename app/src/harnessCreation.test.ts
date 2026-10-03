import { describe, expect, it } from "vitest";
import { withResumeCmds } from "./harnessCmd.ts";
import {
	buildHarness,
	buildRoom,
	claimedSessionIdsOf,
	harnessDisplayName,
	resolveAgentName,
	withCapturedSessionId,
	withoutShellClaim,
	withReplacedSessionId,
	withRespawnedCmd,
	withShellClaim,
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

describe("shell claim helpers (#520)", () => {
	const SHELL = ["/bin/zsh", "-l"];
	const shellRoom = (sessionId = "old"): Room => {
		const r = room(sessionId);
		return { ...r, harnesses: r.harnesses.map((h) => ({ ...h, cmd: SHELL })) };
	};
	const h0 = (rooms: Room[]) => rooms[0]?.harnesses[0];

	it("withShellClaim sets sessionId and the claim together", () => {
		const out = withShellClaim([shellRoom("old")], "s1", "h1", "new");
		expect(h0(out)?.sessionId).toBe("new");
		expect(h0(out)?.shellClaim).toEqual({ sessionId: "new" });
	});

	it("withShellClaim is an identity no-op when already claimed or harness missing", () => {
		const once = withShellClaim([shellRoom()], "s1", "h1", "new");
		expect(withShellClaim(once, "s1", "h1", "new")).toBe(once);
		expect(withShellClaim(once, "nope", "h1", "x")).toBe(once);
		expect(withShellClaim(once, "s1", "nope", "x")).toBe(once);
	});

	it("withShellClaim records a port, omits the key otherwise, and stays no-churn", () => {
		const withPort = withShellClaim([shellRoom()], "s1", "h1", "new", 7);
		expect(h0(withPort)?.shellClaim).toEqual({ sessionId: "new", port: 7 });
		expect(withShellClaim(withPort, "s1", "h1", "new", 7)).toBe(withPort);
		expect(h0(withShellClaim(withPort, "s1", "h1", "new", 8))?.shellClaim?.port).toBe(8);
		const bare = h0(withShellClaim(withPort, "s1", "h1", "new"));
		expect(bare?.shellClaim).toEqual({ sessionId: "new" });
		expect(bare?.shellClaim).not.toHaveProperty("port");
	});

	it("withoutShellClaim removes the key and is a no-op when absent", () => {
		const once = withShellClaim([shellRoom()], "s1", "h1", "new");
		const out = withoutShellClaim(once, "s1", "h1");
		expect(h0(out)).not.toHaveProperty("shellClaim");
		expect(h0(out)?.sessionId).toBe("new");
		expect(withoutShellClaim(out, "s1", "h1")).toBe(out);
		expect(withoutShellClaim(out, "nope", "h1")).toBe(out);
	});

	it("withRespawnedCmd replaces cmd, bumps spawnGen and drops the claim", () => {
		const claimed = withShellClaim([shellRoom()], "s1", "h1", "new");
		const out = withRespawnedCmd(claimed, "s1", "h1", ["claude"], "fresh");
		expect(h0(out)).not.toHaveProperty("shellClaim");
		expect(h0(out)).toMatchObject({ cmd: ["claude"], sessionId: "fresh", spawnGen: 1 });
		const keep = withRespawnedCmd(claimed, "s1", "h1", SHELL);
		expect(h0(keep)?.sessionId).toBe("new");
		expect(h0(keep)?.spawnGen).toBe(1);
	});

	it("claim -> restart rebuild resumes claude; claim -> release keeps the shell", () => {
		const claimed = withShellClaim([shellRoom("old")], "s1", "h1", "sid-1");
		const [claimedRoom] = claimed;
		const [releasedRoom] = withoutShellClaim(claimed, "s1", "h1");
		if (!claimedRoom || !releasedRoom) throw new Error("room missing");
		expect(h0([withResumeCmds(claimedRoom, new Map())])?.cmd).toEqual([
			"claude",
			"--resume",
			"sid-1",
		]);
		expect(h0([withResumeCmds(releasedRoom, new Map())])?.cmd).toEqual(SHELL);
	});
});
