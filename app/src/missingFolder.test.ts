import { describe, expect, it } from "vitest";
import { recoveryOptions, repointRoom } from "./missingFolder.ts";
import type { Harness, Room } from "./types.ts";

const room = (over: Partial<Room> = {}): Room => ({
	id: "r1",
	name: "room",
	task: "task",
	status: "idle",
	badge: 0,
	harnesses: [],
	activeHarnessId: "",
	cwd: "/repo-wt/room",
	...over,
});

const harness = (over: Partial<Harness> = {}): Harness => ({
	id: "h1",
	kind: "claude",
	name: "CC-1",
	status: "idle",
	model: "sonnet-4.5",
	tokens: "0",
	...over,
});

describe("recoveryOptions", () => {
	it("offers recreate when the room is a worktree whose branch is still in the repo", () => {
		const r = room({ branch: "feature-x", repoRoot: "/repo" });
		const info = { exists: true, isRepo: true, branches: [{ name: "feature-x" }] };
		expect(recoveryOptions(r, info)).toEqual({ canRecreateWorktree: true });
	});

	it("refuses with a reason when there is no branch on record", () => {
		const r = room({ repoRoot: "/repo" });
		expect(recoveryOptions(r, undefined)).toEqual({
			canRecreateWorktree: false,
			reason: "This room has no branch on record.",
		});
	});

	it("refuses with a reason when there is no repoRoot on record", () => {
		const r = room({ branch: "feature-x" });
		expect(recoveryOptions(r, undefined)).toEqual({
			canRecreateWorktree: false,
			reason: "This room's repository isn't known.",
		});
	});

	it("refuses when the room's cwd IS the repoRoot (main checkout, not a worktree)", () => {
		const r = room({ branch: "main", repoRoot: "/repo-wt/room" });
		expect(recoveryOptions(r, undefined)).toEqual({
			canRecreateWorktree: false,
			reason: "This room is the main checkout, not a worktree.",
		});
	});

	it("refuses while the repoRoot hasn't been checked yet", () => {
		const r = room({ branch: "feature-x", repoRoot: "/repo" });
		expect(recoveryOptions(r, undefined).canRecreateWorktree).toBe(false);
	});

	it("refuses when the repoRoot is also missing", () => {
		const r = room({ branch: "feature-x", repoRoot: "/repo" });
		const info = { exists: false, isRepo: false, branches: [] };
		expect(recoveryOptions(r, info)).toEqual({
			canRecreateWorktree: false,
			reason: "The repository at /repo is also missing.",
		});
	});

	it("refuses when the repoRoot is no longer a git repository", () => {
		const r = room({ branch: "feature-x", repoRoot: "/repo" });
		const info = { exists: true, isRepo: false, branches: [] };
		expect(recoveryOptions(r, info)).toEqual({
			canRecreateWorktree: false,
			reason: "/repo is no longer a git repository.",
		});
	});

	it("refuses when the branch no longer exists in the repo", () => {
		const r = room({ branch: "feature-x", repoRoot: "/repo" });
		const info = { exists: true, isRepo: true, branches: [{ name: "main" }] };
		expect(recoveryOptions(r, info)).toEqual({
			canRecreateWorktree: false,
			reason: 'Branch "feature-x" no longer exists in the repository.',
		});
	});
});

describe("repointRoom", () => {
	it("moves the room cwd and every harness pointed at the old cwd", () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", cwd: "/old", cmd: ["claude"], sessionId: "sid-1" })],
		});
		const next = repointRoom(r, "/new");
		expect(next.cwd).toBe("/new");
		expect(next.harnesses[0]?.cwd).toBe("/new");
	});

	it("drops sessionId and rebuilds a fresh argv for a resumable harness", () => {
		const r = room({
			cwd: "/old",
			harnesses: [
				harness({
					id: "h1",
					cwd: "/old",
					cmd: ["claude", "--resume", "sid-1"],
					sessionId: "sid-1",
				}),
			],
		});
		const next = repointRoom(r, "/new");
		const h = next.harnesses[0];
		expect(h?.sessionId).toBeUndefined();
		expect(h?.cmd).toEqual(["claude"]);
	});

	it("re-passes the agent when rebuilding a resumable harness's argv", () => {
		const r = room({
			cwd: "/old",
			harnesses: [
				harness({
					id: "h1",
					kind: "opencode",
					cwd: "/old",
					cmd: ["opencode", "--session", "sid-1"],
					sessionId: "sid-1",
					agent: "build",
				}),
			],
		});
		const next = repointRoom(r, "/new", { opencodePorts: new Map([["h1", 4123]]) });
		const h = next.harnesses[0];
		expect(h?.sessionId).toBeUndefined();
		expect(h?.cmd).toEqual([
			"opencode",
			"--port",
			"4123",
			"--hostname",
			"127.0.0.1",
			"--agent",
			"build",
		]);
	});

	it("bumps spawnGen so LiveTerminal remounts even if the argv is unchanged", () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", cwd: "/old", cmd: ["claude"], spawnGen: 2 })],
		});
		const next = repointRoom(r, "/new");
		expect(next.harnesses[0]?.spawnGen).toBe(3);
	});

	it("moves a non-resumable harness (shell) without touching its argv", () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", kind: "byoh", cwd: "/old", cmd: ["bash", "-l"] })],
		});
		const next = repointRoom(r, "/new");
		const h = next.harnesses[0];
		expect(h?.cwd).toBe("/new");
		expect(h?.cmd).toEqual(["bash", "-l"]);
		expect(h?.spawnGen).toBe(1);
	});

	it("leaves a cmd-less harness (files) alone besides moving its cwd", () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", kind: "files", cwd: "/old" })],
		});
		const next = repointRoom(r, "/new");
		expect(next.harnesses[0]).toEqual({ ...r.harnesses[0], cwd: "/new" });
	});

	it("leaves a harness whose cwd never matched the room's cwd untouched", () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", cwd: "/elsewhere", cmd: ["claude"] })],
		});
		const next = repointRoom(r, "/new");
		expect(next.harnesses[0]?.cwd).toBe("/elsewhere");
	});
});
