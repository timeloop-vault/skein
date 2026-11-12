import { describe, expect, it } from "vitest";
import type { FreshCmdFn, FreshCmdInput } from "./harnessArgvApi.ts";
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
	// A fake daemon: records every request, answers per kind (#730).
	const fake = (opencodePort: number | null = 4123) => {
		const calls: FreshCmdInput[] = [];
		const freshCmd: FreshCmdFn = async (input) => {
			calls.push(input);
			return {
				cmd:
					input.kind === "claude"
						? [
								"claude",
								...(input.agent ? ["--agent", input.agent] : []),
								...(input.newHarness ? ["--session-id", "minted"] : []),
							]
						: [input.kind, "fresh"],
				opencodePort: input.kind === "opencode" ? opencodePort : null,
				sessionId: input.kind === "claude" && input.newHarness ? "minted" : null,
				hermesProfile:
					input.kind === "hermes"
						? (input.hermesProfile ?? (input.newHarness ? "skein" : null))
						: null,
			};
		};
		return { calls, freshCmd };
	};

	it("moves the room cwd and every harness pointed at the old cwd", async () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", cwd: "/old", cmd: ["claude"], sessionId: "sid-1" })],
		});
		const { room: next } = await repointRoom(r, "/new", fake().freshCmd);
		expect(next.cwd).toBe("/new");
		expect(next.harnesses[0]?.cwd).toBe("/new");
	});

	it("drops the old sessionId and starts a bare claude (plus --agent when set)", async () => {
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
		const f = fake();
		const { room: next } = await repointRoom(r, "/new", f.freshCmd);
		const h = next.harnesses[0];
		expect(f.calls[0]).toEqual({ kind: "claude" });
		expect(h?.sessionId).toBeUndefined();
		expect(h?.cmd).toEqual(["claude"]);

		const withAgent = room({
			cwd: "/old",
			harnesses: [
				harness({ id: "h2", cwd: "/old", cmd: ["claude"], sessionId: "s", agent: "rev" }),
			],
		});
		const { room: n2 } = await repointRoom(withAgent, "/new", f.freshCmd);
		expect(n2.harnesses[0]?.cmd).toEqual(["claude", "--agent", "rev"]);
		expect(n2.harnesses[0]?.sessionId).toBeUndefined();
	});

	it("re-passes the agent and returns the allocated port", async () => {
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
		const f = fake(4123);
		const { room: next, ports } = await repointRoom(r, "/new", f.freshCmd);
		expect(f.calls[0]).toEqual({ kind: "opencode", agent: "build" });
		expect(next.harnesses[0]?.sessionId).toBeUndefined();
		expect(ports.get("h1")).toBe(4123);
	});

	it("a failed port allocation leaves the port out", async () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", kind: "opencode", cwd: "/old", cmd: ["opencode"] })],
		});
		const { ports } = await repointRoom(r, "/new", fake(null).freshCmd);
		expect(ports.size).toBe(0);
	});

	it("passes a remote spec and a hermes profile through", async () => {
		const remote = { host: "devbox", tool: "claude", session: "s" };
		const r = room({
			cwd: "/old",
			harnesses: [
				harness({ id: "h1", kind: "remote", cwd: "/old", cmd: ["ssh"], remote }),
				harness({ id: "h2", kind: "hermes", cwd: "/old", cmd: ["hermes"], hermesProfile: "p" }),
			],
		});
		const f = fake();
		await repointRoom(r, "/new", f.freshCmd);
		expect(f.calls).toContainEqual({ kind: "remote", remote });
		expect(f.calls).toContainEqual({ kind: "hermes", hermesProfile: "p" });
	});

	it("sends no profile and no newHarness for a hermes record without a profile", async () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h2", kind: "hermes", cwd: "/old", cmd: ["hermes"] })],
		});
		const f = fake();
		await repointRoom(r, "/new", f.freshCmd);
		expect(f.calls).toEqual([{ kind: "hermes" }]);
	});

	it("rejects without a partial result when one daemon call fails", async () => {
		const r = room({
			cwd: "/old",
			harnesses: [
				harness({ id: "h1", kind: "claude", cwd: "/old", cmd: ["claude"] }),
				harness({ id: "h2", kind: "opencode", cwd: "/old", cmd: ["opencode"] }),
			],
		});
		const failing: FreshCmdFn = async (input) => {
			if (input.kind === "opencode") throw new Error("boom");
			return fake().freshCmd(input);
		};
		await expect(repointRoom(r, "/new", failing)).rejects.toThrow("boom");
		expect(r.cwd).toBe("/old");
	});

	it("bumps spawnGen so LiveTerminal remounts even if the argv is unchanged", async () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", cwd: "/old", cmd: ["claude"], spawnGen: 2 })],
		});
		const { room: next } = await repointRoom(r, "/new", fake().freshCmd);
		expect(next.harnesses[0]?.spawnGen).toBe(3);
	});

	it("moves a non-resumable harness (shell) without asking the daemon", async () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", kind: "byoh", cwd: "/old", cmd: ["bash", "-l"] })],
		});
		const f = fake();
		const { room: next } = await repointRoom(r, "/new", f.freshCmd);
		const h = next.harnesses[0];
		expect(f.calls).toHaveLength(0);
		expect(h?.cwd).toBe("/new");
		expect(h?.cmd).toEqual(["bash", "-l"]);
		expect(h?.spawnGen).toBe(1);
	});

	it("leaves a cmd-less harness (files) alone besides moving its cwd", async () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", kind: "files", cwd: "/old" })],
		});
		const { room: next } = await repointRoom(r, "/new", fake().freshCmd);
		expect(next.harnesses[0]).toEqual({ ...r.harnesses[0], cwd: "/new" });
	});

	it("leaves a harness whose cwd never matched the room's cwd untouched", async () => {
		const r = room({
			cwd: "/old",
			harnesses: [harness({ id: "h1", cwd: "/elsewhere", cmd: ["claude"] })],
		});
		const { room: next } = await repointRoom(r, "/new", fake().freshCmd);
		expect(next.harnesses[0]?.cwd).toBe("/elsewhere");
	});
});
