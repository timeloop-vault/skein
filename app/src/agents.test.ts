import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import {
	type AgentInfo,
	type AgentListing,
	checkAgent,
	kindHasAgents,
	listHarnessAgents,
	unknownAgentMessage,
} from "./agents.ts";

const agent = (name: string, over: Partial<AgentInfo> = {}): AgentInfo => ({
	name,
	description: null,
	tools: null,
	model: null,
	source: "user",
	plugin: null,
	allowsReviewTools: true,
	...over,
});

const listing = (names: string[], degraded: string | null = null): AgentListing => ({
	agents: names.map((n) => agent(n)),
	degraded,
	unsupported: false,
});

describe("kindHasAgents", () => {
	it("is the capability, not a kind comparison", () => {
		expect(kindHasAgents("claude")).toBe(true);
		expect(kindHasAgents("opencode")).toBe(true);
		// copilot is the case a `kind === "claude"` test gets wrong in the
		// other direction: a managed program with no agent concept.
		expect(kindHasAgents("copilot")).toBe(false);
		expect(kindHasAgents("byoh")).toBe(false);
		expect(kindHasAgents("files")).toBe(false);
	});
});

describe("checkAgent (daemon verdict)", () => {
	beforeEach(() => {
		invokeMock.mockReset();
	});

	it("no name: ok, and the CLI is not asked", async () => {
		expect(await checkAgent("claude", "/w", undefined)).toEqual({ kind: "ok" });
		expect(await checkAgent("claude", "/w", "  ")).toEqual({ kind: "ok" });
		expect(invokeMock).not.toHaveBeenCalled();
	});

	it("passes the name as `check` on one list_harness_agents call", async () => {
		invokeMock.mockResolvedValue({ ...listing(["a"]), verdict: { kind: "ok" } });
		expect(await checkAgent("claude", "/w", "a")).toEqual({ kind: "ok" });
		expect(invokeMock).toHaveBeenCalledTimes(1);
		expect(invokeMock).toHaveBeenCalledWith("list_harness_agents", {
			kind: "claude",
			cwd: "/w",
			check: "a",
		});
	});

	it("relays the daemon's unknown and unverified verdicts", async () => {
		invokeMock.mockResolvedValue({ ...listing(["a"]), verdict: { kind: "unknown", agent: "b" } });
		expect((await checkAgent("claude", "/w", "b")).kind).toBe("unknown");
		const v = { kind: "unverified", agent: "b", why: "x" };
		invokeMock.mockResolvedValue({ ...listing([], "x"), verdict: v });
		expect(await checkAgent("claude", "/w", "b")).toEqual(v);
	});

	it("a failed invoke or a kind without agents is unverified, never unknown", async () => {
		invokeMock.mockImplementation(() => Promise.reject(new Error("bridge gone")));
		expect((await checkAgent("claude", "/w", "b")).kind).toBe("unverified");
		expect((await checkAgent("copilot", "/w", "b")).kind).toBe("unverified");
	});

	it("listHarnessAgents without check sends no check key", async () => {
		invokeMock.mockResolvedValue(listing([]));
		await listHarnessAgents("claude", "/w");
		expect(invokeMock).toHaveBeenCalledWith("list_harness_agents", { kind: "claude", cwd: "/w" });
	});
});

describe("unknownAgentMessage", () => {
	it("names the agent and says why nothing spawned", () => {
		const msg = unknownAgentMessage("reviewer", "claude");
		expect(msg).toContain('"reviewer"');
		expect(msg).toContain("Claude Code");
		// The *reason* is the load-bearing half: the user is looking at a
		// harness that did not start, and the alternative was one that
		// started as something else without saying so.
		expect(msg).toContain("without saying so");
	});
});
