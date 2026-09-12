import { describe, expect, it } from "vitest";
import { cmdForKind, resumeCmd, unarchiveRoomTransform, withResumeCmds } from "./harnessCmd.ts";
import type { Harness, HarnessKind, Room } from "./types.ts";

const SID = "3c8c4693-3838-46ab-8680-3e60df6fdefe";
const SHELL = ["/bin/zsh", "-l"];

const harness = (kind: HarnessKind, over: Partial<Harness> = {}): Harness => ({
	id: "h1",
	kind,
	name: "H",
	status: "idle",
	model: "",
	tokens: "",
	...over,
});

const room = (harnesses: Harness[], over: Partial<Room> = {}): Room => ({
	id: "r1",
	name: "R",
	task: "",
	status: "idle",
	badge: 0,
	harnesses,
	activeHarnessId: harnesses[0]?.id ?? "",
	...over,
});

const ports = (entries: [string, number][] = []) => new Map(entries);

describe("cmdForKind", () => {
	it("pre-allocates Claude's session id when given one", () => {
		expect(cmdForKind("claude", SHELL, SID)).toEqual(["claude", "--session-id", SID]);
		expect(cmdForKind("claude", SHELL)).toEqual(["claude"]);
	});

	it("pins opencode's embedded-server port, and degrades without one", () => {
		expect(cmdForKind("opencode", SHELL, undefined, 4096)).toEqual([
			"opencode",
			"--port",
			"4096",
			"--hostname",
			"127.0.0.1",
		]);
		// No port = the L2c-2 SSE adapter can't attach, but the harness
		// still runs. Silently letting opencode pick port 0 would look
		// identical and break telemetry.
		expect(cmdForKind("opencode", SHELL)).toEqual(["opencode"]);
	});

	it("builds the non-resumable kinds", () => {
		expect(cmdForKind("copilot", SHELL)).toEqual(["gh", "copilot", "suggest"]);
		expect(cmdForKind("byoh", SHELL)).toEqual(SHELL);
		expect(cmdForKind("byoh", [])).toEqual(["pwsh.exe"]);
		expect(cmdForKind("files", SHELL)).toEqual([]);
	});
});

describe("resumeCmd — Claude", () => {
	it("rewrites a fresh spawn into a resume", () => {
		const h = harness("claude", { cmd: ["claude", "--session-id", SID], sessionId: SID });
		expect(resumeCmd(h)).toEqual(["claude", "--resume", SID]);
	});

	it("falls back to Claude's picker with no session id", () => {
		expect(resumeCmd(harness("claude", { cmd: ["claude"] }))).toEqual(["claude", "--resume"]);
	});

	it("is idempotent, so every boot and reopen can run it", () => {
		const h = harness("claude", { cmd: ["claude", "--resume", SID], sessionId: SID });
		expect(resumeCmd(h)).toEqual(["claude", "--resume", SID]);
		expect(resumeCmd({ ...h, cmd: resumeCmd(h) })).toEqual(["claude", "--resume", SID]);
	});

	// The #153 / #170 regression guard, and the reason this module
	// exists. The old implementation matched a rebuildable argv by exact
	// length (1 or 3), so any additional spawn flag — `--agent <name>`
	// was the next one, and it shipped in #247 — made a fresh cmd fall
	// through unchanged. Boot then respawned `--session-id` against a
	// session that already existed and Claude died with "Session ID is
	// already in use". Length must not enter into it.
	//
	// Note what these cases assert about #247 specifically: the argv
	// carries `--agent reviewer` and the *record* does not, and the
	// rebuild drops it. The record is the authority, not the argv.

	it("rebuilds regardless of how many spawn flags the argv carries", () => {
		for (const cmd of [
			["claude", "--session-id", SID, "--agent", "reviewer"],
			["claude", "--agent", "reviewer"],
			["claude", "--session-id", SID, "--model", "opus", "--permission-mode", "plan"],
		]) {
			const h = harness("claude", { cmd, sessionId: SID });
			expect(resumeCmd(h), cmd.join(" ")).toEqual(["claude", "--resume", SID]);
		}
	});

	it("leaves a shell-swapped harness alone", () => {
		// Enter-for-shell after an exit (LiveTerminal) replaces the argv
		// wholesale. Rebuilding it would resurrect a retired harness.
		const h = harness("claude", { cmd: ["pwsh.exe"], sessionId: SID });
		expect(resumeCmd(h)).toEqual(["pwsh.exe"]);
	});
});

describe("resumeCmd — opencode", () => {
	it("bakes in the freshly allocated port and the stored session", () => {
		const h = harness("opencode", {
			cmd: ["opencode", "--port", "4096", "--hostname", "127.0.0.1"],
			sessionId: "ses_abc",
		});
		expect(resumeCmd(h, 5150)).toEqual([
			"opencode",
			"--port",
			"5150",
			"--hostname",
			"127.0.0.1",
			"--session",
			"ses_abc",
		]);
	});

	it("resumes most-recent-in-cwd when the session id was never captured", () => {
		const h = harness("opencode", { cmd: ["opencode"] });
		expect(resumeCmd(h, 5150)).toEqual([
			"opencode",
			"--port",
			"5150",
			"--hostname",
			"127.0.0.1",
			"--continue",
		]);
	});

	it("still resumes when port allocation failed", () => {
		const h = harness("opencode", { cmd: ["opencode"], sessionId: "ses_abc" });
		expect(resumeCmd(h)).toEqual(["opencode", "--session", "ses_abc"]);
	});

	it("is idempotent apart from the port, which must change", () => {
		const h = harness("opencode", { cmd: ["opencode"], sessionId: "ses_abc" });
		const once = resumeCmd(h, 5150);
		expect(resumeCmd({ ...h, cmd: once }, 6200)).toEqual([
			"opencode",
			"--port",
			"6200",
			"--hostname",
			"127.0.0.1",
			"--session",
			"ses_abc",
		]);
	});

	it("leaves a shell-swapped harness alone", () => {
		const h = harness("opencode", { cmd: ["pwsh.exe"], sessionId: "ses_abc" });
		expect(resumeCmd(h)).toEqual(["pwsh.exe"]);
	});
});

describe("resumeCmd — kinds without a resume concept", () => {
	it("passes them through untouched (#184 capability gate)", () => {
		const copilot = harness("copilot", { cmd: ["gh", "copilot", "suggest"] });
		expect(resumeCmd(copilot)).toEqual(["gh", "copilot", "suggest"]);
		const shell = harness("byoh", { cmd: SHELL });
		expect(resumeCmd(shell)).toEqual(SHELL);
		// A `files` harness has no process at all. Even with a stray
		// sessionId on the record it must not gain an argv.
		const files = harness("files", { sessionId: SID });
		expect(resumeCmd(files)).toEqual([]);
	});
});

describe("withResumeCmds", () => {
	it("rewrites per harness, keyed by the harness's own port", () => {
		const a = harness("opencode", { id: "a", cmd: ["opencode"], sessionId: "ses_a" });
		const b = harness("opencode", { id: "b", cmd: ["opencode"], sessionId: "ses_b" });
		const out = withResumeCmds(
			room([a, b]),
			ports([
				["a", 4096],
				["b", 4097],
			]),
		);
		expect(out.harnesses[0]?.cmd).toContain("4096");
		expect(out.harnesses[1]?.cmd).toContain("4097");
	});

	it("leaves a cmd-less harness without a cmd", () => {
		// `cmd: undefined` must stay *absent*, not become []; an empty
		// argv would be a spawnable-looking record.
		const out = withResumeCmds(room([harness("files")]), ports());
		expect(out.harnesses[0]).not.toHaveProperty("cmd");
	});
});

describe("unarchiveRoomTransform", () => {
	it("drops the archived stamp and resumes every harness at once", () => {
		const h = harness("claude", { cmd: ["claude", "--session-id", SID], sessionId: SID });
		const out = unarchiveRoomTransform(room([h], { archived: 1_700_000_000_000 }), ports());
		// Both halves, together: #170 shipped a third un-archive path
		// that did the first and forgot the second.
		expect(out).not.toHaveProperty("archived");
		expect(out.harnesses[0]?.cmd).toEqual(["claude", "--resume", SID]);
	});

	it("preserves the rest of the room", () => {
		const out = unarchiveRoomTransform(
			room([harness("claude", { cmd: ["claude"] })], {
				archived: 1,
				branch: "feat/x",
				repo: "skein",
				task: "t",
			}),
			ports(),
		);
		expect(out.branch).toBe("feat/x");
		expect(out.repo).toBe("skein");
		expect(out.task).toBe("t");
	});
});

// ── #247: the agent, across every kind × (agent / no agent) ────────
//
// The acceptance criterion that needs a test rather than a look is
// round-tripping: a selection has to survive a restart *and* an archive
// → reopen, both of which go through `resumeCmd`, and neither of which
// the fresh-spawn argv can vouch for.

describe("cmdForKind — agent", () => {
	it("appends --agent for the kinds that take one", () => {
		expect(cmdForKind("claude", SHELL, SID, undefined, "reviewer")).toEqual([
			"claude",
			"--session-id",
			SID,
			"--agent",
			"reviewer",
		]);
		// No session id is the picker-fallback shape; the agent still applies.
		expect(cmdForKind("claude", SHELL, undefined, undefined, "reviewer")).toEqual([
			"claude",
			"--agent",
			"reviewer",
		]);
		expect(cmdForKind("opencode", SHELL, undefined, 4096, "plan")).toEqual([
			"opencode",
			"--port",
			"4096",
			"--hostname",
			"127.0.0.1",
			"--agent",
			"plan",
		]);
		// Port allocation failed: the agent is not collateral damage.
		expect(cmdForKind("opencode", SHELL, undefined, undefined, "plan")).toEqual([
			"opencode",
			"--agent",
			"plan",
		]);
	});

	it("omits the flag entirely with no agent — the tool's own default", () => {
		expect(cmdForKind("claude", SHELL, SID)).not.toContain("--agent");
		expect(cmdForKind("opencode", SHELL, undefined, 4096)).not.toContain("--agent");
	});

	it("treats a blank name as no agent, not as an empty flag", () => {
		// `--agent ""` is refused by Claude at spawn, and the only way to
		// get one here is a hand-edited blob.
		expect(cmdForKind("claude", SHELL, SID, undefined, "   ")).toEqual([
			"claude",
			"--session-id",
			SID,
		]);
	});
});

describe("resumeCmd — agent", () => {
	it("re-passes the record's agent for claude", () => {
		// `claude --resume <sid>` alone restores the agent the session
		// *started* as (measured on #219), so a record that says
		// otherwise has to say it out loud on every resume.
		const h = harness("claude", {
			cmd: ["claude", "--session-id", SID, "--agent", "reviewer"],
			sessionId: SID,
			agent: "reviewer",
		});
		expect(resumeCmd(h)).toEqual(["claude", "--resume", SID, "--agent", "reviewer"]);
	});

	it("re-passes the record's agent for opencode, with a fresh port", () => {
		const h = harness("opencode", {
			cmd: ["opencode", "--port", "4096", "--hostname", "127.0.0.1", "--agent", "plan"],
			sessionId: "ses_abc",
			agent: "plan",
		});
		expect(resumeCmd(h, 5150)).toEqual([
			"opencode",
			"--port",
			"5150",
			"--hostname",
			"127.0.0.1",
			"--session",
			"ses_abc",
			"--agent",
			"plan",
		]);
	});

	it("is idempotent with an agent, so every boot can run it", () => {
		const h = harness("claude", { cmd: ["claude"], sessionId: SID, agent: "reviewer" });
		const once = resumeCmd(h);
		expect(resumeCmd({ ...h, cmd: once })).toEqual(once);
	});

	it("keeps a shell-swapped harness on its shell, agent or not", () => {
		// The record outlives the process it described: Enter-for-shell
		// leaves `agent` set, and the shell must not grow a flag it does
		// not understand.
		const h = harness("claude", { cmd: ["pwsh.exe"], sessionId: SID, agent: "reviewer" });
		expect(resumeCmd(h)).toEqual(["pwsh.exe"]);
	});

	it("ignores an agent on a kind that has no agent concept", () => {
		// Belt and braces: `createHarnessInRoom` gates on the capability
		// before the name reaches the record, but a hand-edited blob or a
		// future kind must not be able to hand `--agent` to a shell.
		const copilot = harness("copilot", { cmd: ["gh", "copilot", "suggest"], agent: "reviewer" });
		expect(resumeCmd(copilot)).toEqual(["gh", "copilot", "suggest"]);
		const shell = harness("byoh", { cmd: SHELL, agent: "reviewer" });
		expect(resumeCmd(shell)).toEqual(SHELL);
	});
});

describe("#247 round-trip: the selection survives restart and reopen", () => {
	const fresh = (kind: HarnessKind, agent: string) =>
		harness(kind, {
			cmd: cmdForKind(kind, SHELL, kind === "claude" ? SID : undefined, 4096, agent),
			...(kind === "claude" ? { sessionId: SID } : { sessionId: "ses_abc" }),
			agent,
		});

	it("survives a Skein restart for claude and opencode", () => {
		for (const [kind, agent] of [
			["claude", "reviewer"],
			["opencode", "plan"],
		] as const) {
			const out = withResumeCmds(room([fresh(kind, agent)]), ports([["h1", 5150]]));
			const cmd = out.harnesses[0]?.cmd ?? [];
			expect(cmd, kind).toContain("--agent");
			expect(cmd[cmd.indexOf("--agent") + 1], kind).toBe(agent);
			// The #153 guard, restated where the agent could reintroduce
			// it: a resumed argv must never carry --session-id.
			expect(cmd, kind).not.toContain("--session-id");
			expect(out.harnesses[0]?.agent, kind).toBe(agent);
		}
	});

	it("survives archive → reopen", () => {
		const out = unarchiveRoomTransform(
			room([fresh("claude", "reviewer")], { archived: 1_700_000_000_000 }),
			ports(),
		);
		expect(out.harnesses[0]?.cmd).toEqual(["claude", "--resume", SID, "--agent", "reviewer"]);
		expect(out.harnesses[0]?.agent).toBe("reviewer");
	});
});
