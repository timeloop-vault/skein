import { describe, expect, it, vi } from "vitest";
import type { AgentVerdict } from "./agents.ts";
import { agentResolves } from "./terminalSpawnAgent.ts";

const run = async (
	verdict: AgentVerdict,
	over: { cmd?: string[]; agent?: string | undefined } = {},
) => {
	const check = vi.fn(async () => verdict);
	const out = { write: vi.fn() };
	const onRefused = vi.fn();
	const ok = await agentResolves({
		agent: "agent" in over ? over.agent : "ghost",
		cmdToSpawn: over.cmd ?? ["claude", "--agent", "ghost"],
		harnessKind: "claude",
		harnessId: "h1",
		cwd: "/w",
		out,
		isCancelled: () => false,
		onRefused,
		check,
	});
	return { ok, check, out, onRefused };
};

describe("agentResolves (#730: the daemon's verdict)", () => {
	it("unknown blocks the spawn and says why", async () => {
		const r = await run({ kind: "unknown", agent: "ghost" });
		expect(r.ok).toBe(false);
		expect(r.onRefused).toHaveBeenCalled();
		expect(r.out.write.mock.calls.join("")).toContain('agent "ghost"');
	});

	it("unverified does not block", async () => {
		vi.spyOn(console, "warn").mockImplementation(() => {});
		const r = await run({ kind: "unverified", agent: "ghost", why: "cli down" });
		expect(r.ok).toBe(true);
		expect(r.onRefused).not.toHaveBeenCalled();
	});

	it("ok does not block", async () => {
		expect((await run({ kind: "ok" })).ok).toBe(true);
	});

	it("no --agent in the argv (a shell) or no agent: no check at all", async () => {
		const shell = await run({ kind: "unknown", agent: "ghost" }, { cmd: ["pwsh.exe"] });
		expect(shell.ok).toBe(true);
		expect(shell.check).not.toHaveBeenCalled();
		const none = await run({ kind: "unknown", agent: "ghost" }, { agent: undefined });
		expect(none.ok).toBe(true);
		expect(none.check).not.toHaveBeenCalled();
	});
});
