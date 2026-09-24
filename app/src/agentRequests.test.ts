import { describe, expect, it } from "vitest";
import {
	parseCreateArgs,
	parseResolveArgs,
	resolveAgent,
	resolveKind,
	specFromCreateArgs,
} from "./agentRequests.ts";
import type { AgentListing } from "./agents.ts";
import { NO_AGENTS } from "./agents.ts";
import type { DefaultAgents, FolderDefaults } from "./prefs.ts";

const folderDefaults = (over: Partial<FolderDefaults> = {}): FolderDefaults => ({
	baseBranch: "main",
	harness: "opencode",
	branchMode: "worktree",
	lastUsed: 0,
	...over,
});

const listing = (over: Partial<AgentListing> = {}): AgentListing => ({
	agents: [
		{
			name: "reviewer",
			description: null,
			tools: null,
			model: null,
			source: "project",
			plugin: null,
			allowsReviewTools: true,
		},
	],
	degraded: null,
	unsupported: false,
	...over,
});

describe("parseResolveArgs", () => {
	it("requires a non-empty path", () => {
		expect(parseResolveArgs({})).toEqual({ ok: false, error: "path is required" });
		expect(parseResolveArgs({ path: "  " })).toEqual({ ok: false, error: "path is required" });
	});

	it("rejects a non-object payload", () => {
		expect(parseResolveArgs("nope")).toEqual({ ok: false, error: "args must be an object" });
		expect(parseResolveArgs(null)).toEqual({ ok: false, error: "args must be an object" });
	});

	it("accepts path alone, kind/agent absent", () => {
		expect(parseResolveArgs({ path: "/repo" })).toEqual({ ok: true, value: { path: "/repo" } });
	});

	it("carries kind and agent through when given", () => {
		expect(parseResolveArgs({ path: "/repo", kind: "claude", agent: "reviewer" })).toEqual({
			ok: true,
			value: { path: "/repo", kind: "claude", agent: "reviewer" },
		});
	});

	it("rejects a non-string kind or agent", () => {
		expect(parseResolveArgs({ path: "/repo", kind: 3 })).toEqual({
			ok: false,
			error: "kind must be a string",
		});
		expect(parseResolveArgs({ path: "/repo", agent: 3 })).toEqual({
			ok: false,
			error: "agent must be a string",
		});
	});

	// Regression for the bug seen live: Rust's `serde_json::json!` macro
	// serializes an absent `Option<String>` as JSON `null`, never as a
	// missing key — this is the exact payload `create_room`'s first
	// round trip sends for a minimal `{task}`-only call.
	it("treats explicit null the same as absent, matching Rust's json! output", () => {
		expect(parseResolveArgs({ path: "/repo", kind: null, agent: null })).toEqual({
			ok: true,
			value: { path: "/repo" },
		});
	});
});

describe("resolveKind", () => {
	it("falls back to the folder's remembered harness when none is requested", () => {
		expect(resolveKind(undefined, folderDefaults({ harness: "opencode" }))).toEqual({
			ok: true,
			value: "opencode",
		});
	});

	it("falls back to claude with no folder memory either", () => {
		expect(resolveKind(undefined, undefined)).toEqual({ ok: true, value: "claude" });
	});

	it("a requested kind overrides folder memory", () => {
		expect(resolveKind("copilot", folderDefaults({ harness: "opencode" }))).toEqual({
			ok: true,
			value: "copilot",
		});
	});

	it("rejects an unknown kind", () => {
		expect(resolveKind("gpt-nonsense", undefined)).toEqual({
			ok: false,
			error: 'unknown harness kind "gpt-nonsense"',
		});
	});
});

describe("resolveAgent", () => {
	const defaults: DefaultAgents = {};

	it("no agent requested, no folder/default memory: ok with null", () => {
		expect(resolveAgent(undefined, "claude", undefined, defaults, NO_AGENTS)).toEqual({
			ok: true,
			value: null,
		});
	});

	it("a requested agent the listing vouches for: ok with the name", () => {
		expect(resolveAgent("reviewer", "claude", undefined, defaults, listing())).toEqual({
			ok: true,
			value: "reviewer",
		});
	});

	it("a requested agent absent from an authoritative listing: unknown, blocks", () => {
		const result = resolveAgent("ghost", "claude", undefined, defaults, listing());
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.error).toContain('agent "ghost"');
	});

	it("a requested agent against a degraded listing: unverified, never blocks", () => {
		expect(
			resolveAgent("ghost", "claude", undefined, defaults, {
				agents: [],
				degraded: "claude is not on PATH",
				unsupported: false,
			}),
		).toEqual({ ok: true, value: "ghost" });
	});

	it("falls back through the folder's own remembered agent for the same kind", () => {
		const folder = folderDefaults({ harness: "claude", agent: "reviewer" });
		expect(resolveAgent(undefined, "claude", folder, defaults, listing())).toEqual({
			ok: true,
			value: "reviewer",
		});
	});
});

describe("parseCreateArgs", () => {
	const valid = {
		path: "/repo",
		branchMode: "worktree" as const,
		task: "fix the thing",
		kind: "claude" as const,
		agent: null,
		createdBy: { roomId: "s1", harnessId: "h1" },
		requesterRoomName: "backlog",
	};

	it("accepts a fully-formed request", () => {
		expect(parseCreateArgs(valid)).toEqual({ ok: true, value: valid });
	});

	it("carries branch/baseBranch through only when given", () => {
		const withBranch = { ...valid, branch: "feat/x", baseBranch: "main" };
		expect(parseCreateArgs(withBranch)).toEqual({ ok: true, value: withBranch });
	});

	it("rejects a bad branchMode", () => {
		expect(parseCreateArgs({ ...valid, branchMode: "sideways" })).toEqual({
			ok: false,
			error: 'branchMode must be "worktree" or "current"',
		});
	});

	it("rejects an unknown kind", () => {
		expect(parseCreateArgs({ ...valid, kind: "not-a-kind" })).toEqual({
			ok: false,
			error: 'unknown harness kind "not-a-kind"',
		});
	});

	it("rejects an agent that is neither a string nor null", () => {
		expect(parseCreateArgs({ ...valid, agent: 42 })).toEqual({
			ok: false,
			error: "agent must be a string or null",
		});
	});

	it("accepts a string agent", () => {
		expect(parseCreateArgs({ ...valid, agent: "reviewer" })).toEqual({
			ok: true,
			value: { ...valid, agent: "reviewer" },
		});
	});

	it("rejects a missing or malformed createdBy", () => {
		expect(parseCreateArgs({ ...valid, createdBy: undefined })).toEqual({
			ok: false,
			error: "createdBy must be {roomId, harnessId}",
		});
		expect(parseCreateArgs({ ...valid, createdBy: { roomId: "s1" } })).toEqual({
			ok: false,
			error: "createdBy must be {roomId, harnessId}",
		});
		// `roomId` empty is still a reject — every other verb in this API
		// scopes on it, and an empty one can never resolve.
		expect(parseCreateArgs({ ...valid, createdBy: { roomId: "", harnessId: "h1" } })).toEqual({
			ok: false,
			error: "createdBy must be {roomId, harnessId}",
		});
	});

	// Regression: Rust's second round trip sends `branch`/`baseBranch` as
	// explicit `null` whenever the agent omitted them — same shape as
	// the resolve request above.
	it("treats null branch/baseBranch the same as absent, matching Rust's json! output", () => {
		const args = { ...valid, branch: null, baseBranch: null };
		expect(parseCreateArgs(args)).toEqual({ ok: true, value: valid });
	});

	it("accepts an empty harnessId — the caller's own X-Skein-Harness header is optional", () => {
		// Rust sends "" rather than omitting the key or sending null when
		// the create_room call carried no X-Skein-Harness.
		const args = { ...valid, createdBy: { roomId: "s1", harnessId: "" } };
		expect(parseCreateArgs(args)).toEqual({ ok: true, value: args });
	});

	it("rejects an empty task or requesterRoomName", () => {
		expect(parseCreateArgs({ ...valid, task: "  " })).toEqual({
			ok: false,
			error: "task is required",
		});
		expect(parseCreateArgs({ ...valid, requesterRoomName: "" })).toEqual({
			ok: false,
			error: "requesterRoomName is required",
		});
	});
});

describe("specFromCreateArgs", () => {
	it("carries agent/branch/baseBranch only when present", () => {
		const args = {
			path: "/repo",
			branchMode: "worktree" as const,
			task: "fix the thing",
			kind: "claude" as const,
			agent: null,
			createdBy: { roomId: "s1", harnessId: "h1" },
			requesterRoomName: "backlog",
		};
		expect(specFromCreateArgs(args, "skein/{slug}")).toEqual({
			folder: "/repo",
			task: "fix the thing",
			harness: "claude",
			branchMode: "worktree",
			branchTemplate: "skein/{slug}",
		});
		expect(
			specFromCreateArgs(
				{ ...args, agent: "reviewer", branch: "feat/x", baseBranch: "main" },
				"skein/{slug}",
			),
		).toEqual({
			folder: "/repo",
			task: "fix the thing",
			harness: "claude",
			agent: "reviewer",
			branchMode: "worktree",
			branch: "feat/x",
			baseBranch: "main",
			branchTemplate: "skein/{slug}",
		});
	});
});
