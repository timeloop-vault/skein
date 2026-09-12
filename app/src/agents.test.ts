import { describe, expect, it } from "vitest";
import {
	type AgentInfo,
	type AgentListing,
	NO_AGENTS,
	kindHasAgents,
	unknownAgentMessage,
	validateAgent,
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

describe("validateAgent", () => {
	it("accepts no agent at all — the tool's own default is a choice", () => {
		expect(validateAgent(undefined, listing(["reviewer"]))).toEqual({ kind: "ok" });
		expect(validateAgent("", listing(["reviewer"]))).toEqual({ kind: "ok" });
		expect(validateAgent("  ", listing(["reviewer"]))).toEqual({ kind: "ok" });
	});

	it("accepts a name the CLI vouched for", () => {
		expect(validateAgent("reviewer", listing(["coder", "reviewer"]))).toEqual({ kind: "ok" });
	});

	it("rejects a name an authoritative list does not have", () => {
		expect(validateAgent("reviewer", listing(["coder"]))).toEqual({
			kind: "unknown",
			agent: "reviewer",
		});
	});

	it("matches exactly — namespaced names are not the same agent", () => {
		// `pr-review-toolkit:code-reviewer` and `code-reviewer` are two
		// different things to the CLI, and `--agent` takes the full name.
		expect(validateAgent("code-reviewer", listing(["pr-review-toolkit:code-reviewer"]))).toEqual({
			kind: "unknown",
			agent: "code-reviewer",
		});
	});

	it("is case-sensitive, because the CLI is", () => {
		// This box has both `Explore` and `explore`, with different tools.
		expect(validateAgent("explore", listing(["Explore"]))).toEqual({
			kind: "unknown",
			agent: "explore",
		});
	});

	it("never rejects on a degraded list — absence proves nothing there", () => {
		// The whole point: an unrunnable CLI must not strand every harness
		// that names an agent. The spawn itself still fails loudly if the
		// name really is gone.
		expect(validateAgent("reviewer", listing([], "claude: program not found"))).toEqual({
			kind: "unverified",
			agent: "reviewer",
			why: "claude: program not found",
		});
		expect(validateAgent("reviewer", listing(["coder"], "output unparseable"))).toEqual({
			kind: "unverified",
			agent: "reviewer",
			why: "output unparseable",
		});
	});

	it("treats a listing nobody fetched as unverified, not as empty", () => {
		// NO_AGENTS is what a kind without agents gets, and what a caller
		// holds before the first fetch lands. Reading it as "the list is
		// empty, so the name is gone" would block every spawn.
		expect(validateAgent("reviewer", NO_AGENTS).kind).toBe("unverified");
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
