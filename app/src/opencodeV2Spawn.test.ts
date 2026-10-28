import { describe, expect, it, vi } from "vitest";
import {
	createSidecarDeathRecord,
	matchSidecarExit,
	sidecarStoppedLine,
} from "./opencodeSidecarExit.ts";
import { type PrepareDeps, type PrepareOpts, prepareOpencodeSpawn } from "./opencodeV2Spawn.ts";
import type { OpencodeVersion } from "./types.ts";

const V1_CMD = ["opencode", "--port", "4100", "--hostname", "127.0.0.1", "--session", "ses_old"];

const mk = (version: OpencodeVersion | null, over: Partial<PrepareDeps> = {}) => {
	const deps: PrepareDeps = {
		version: vi.fn(async () => version),
		start: vi.fn(async () => ({
			url: "http://127.0.0.1:5000",
			port: 5000,
			sessionId: "ses_new",
			generation: 7,
		})),
		stop: vi.fn(async () => {}),
		...over,
	};
	const setVersion = vi.fn();
	const setSessionId = vi.fn();
	const opts = (over2: Partial<PrepareOpts> = {}): PrepareOpts => ({
		kind: "opencode",
		cmd: V1_CMD,
		roomId: "r1",
		harnessId: "h1",
		cwd: "/w",
		sessionId: undefined,
		agent: undefined,
		envProfile: undefined,
		setOpencodeVersion: setVersion,
		setSessionId,
		isCancelled: () => false,
		deps,
		...over2,
	});
	return { deps, setVersion, setSessionId, opts };
};

describe("prepareOpencodeSpawn (#676)", () => {
	it("non-opencode kinds are untouched and never probed", async () => {
		const t = mk({ major: 2, version: "2.0.0" });
		const out = await prepareOpencodeSpawn(t.opts({ kind: "claude", cmd: ["claude"] }));
		expect(out).toEqual({ argv: ["claude"], v2: false });
		expect(t.deps.version).not.toHaveBeenCalled();
	});

	it("v1 returns the stored cmd, records the version, never starts a sidecar", async () => {
		const t = mk({ major: 1, version: "1.4.0" });
		const out = await prepareOpencodeSpawn(t.opts());
		expect(out).toEqual({ argv: V1_CMD, v2: false });
		expect(out?.argv).toBe(V1_CMD);
		expect(t.setVersion).toHaveBeenCalledWith({ major: 1, version: "1.4.0" });
		expect(t.deps.start).not.toHaveBeenCalled();
	});

	it("an unknown (null) version is v1", async () => {
		const t = mk(null);
		const out = await prepareOpencodeSpawn(t.opts());
		expect(out?.v2).toBe(false);
		expect(t.setVersion).toHaveBeenCalledWith(null);
		expect(t.deps.start).not.toHaveBeenCalled();
	});

	it("v2 starts the sidecar with the record's session and agent", async () => {
		const t = mk({ major: 2, version: "2.0.0" });
		const out = await prepareOpencodeSpawn(
			t.opts({ sessionId: "ses_rec", agent: "plan", envProfile: "p" }),
		);
		expect(t.deps.start).toHaveBeenCalledWith({
			roomId: "r1",
			harnessId: "h1",
			cwd: "/w",
			agent: "plan",
			sessionId: "ses_rec",
			envProfile: "p",
		});
		expect(out).toEqual({
			argv: ["opencode", "--server", "http://127.0.0.1:5000", "--session", "ses_new"],
			v2: true,
			sidecarGeneration: 7,
		});
		expect(t.setSessionId).toHaveBeenCalledWith("ses_new");
	});

	it("v2 sends nulls for an absent or blank session/agent and skips an unchanged id", async () => {
		const t = mk({ major: 2, version: "2.0.0" });
		await prepareOpencodeSpawn(t.opts({ agent: "  ", sessionId: "" }));
		expect(t.deps.start).toHaveBeenCalledWith(
			expect.objectContaining({ agent: null, sessionId: null, envProfile: null }),
		);
		const same = mk({ major: 2, version: "2.0.0" });
		await prepareOpencodeSpawn(same.opts({ sessionId: "ses_new" }));
		expect(same.setSessionId).not.toHaveBeenCalled();
	});

	it("cancellation after the sidecar started stops it and resolves null", async () => {
		const t = mk({ major: 2, version: "2.0.0" });
		let calls = 0;
		const out = await prepareOpencodeSpawn(t.opts({ isCancelled: () => ++calls > 1 }));
		expect(out).toBeNull();
		expect(t.deps.stop).toHaveBeenCalledWith("h1", 7);
		expect(t.setSessionId).not.toHaveBeenCalled();
	});

	it("cancellation before the probe result drops the version", async () => {
		const t = mk({ major: 2, version: "2.0.0" });
		const out = await prepareOpencodeSpawn(t.opts({ isCancelled: () => true }));
		expect(out).toBeNull();
		expect(t.setVersion).not.toHaveBeenCalled();
		expect(t.deps.start).not.toHaveBeenCalled();
	});

	it("a start error propagates", async () => {
		const t = mk(
			{ major: 2, version: "2.0.0" },
			{ start: vi.fn(async () => Promise.reject(new Error("boom"))) },
		);
		await expect(prepareOpencodeSpawn(t.opts())).rejects.toThrow("boom");
	});
});

describe("sidecar death record", () => {
	const p = { harnessId: "h1", generation: 3, code: 1 };
	it("matches this harness's current generation only", () => {
		expect(matchSidecarExit(p, "h1", 3)).toEqual({ code: 1 });
		expect(matchSidecarExit(p, "h2", 3)).toBeUndefined();
		expect(matchSidecarExit(p, "h1", 4)).toBeUndefined();
		expect(matchSidecarExit(p, "h1", undefined)).toBeUndefined();
		expect(matchSidecarExit({ ...p, code: null }, "h1", 3)).toEqual({ code: null });
	});
	it("formats the line, with ? for an unknown code", () => {
		expect(sidecarStoppedLine(1)).toContain("[opencode server stopped (exit 1)");
		expect(sidecarStoppedLine(null)).toContain("exit ?");
	});
	it("yields nothing until a death is noted, then once", () => {
		const r = createSidecarDeathRecord(() => {});
		r.note(2);
		expect(r.take()).toContain("exit 2");
		expect(r.take()).toBe("");
	});
});
