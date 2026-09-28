import { describe, expect, it } from "vitest";
import {
	decideCloseHarness,
	decideCloseRoom,
	decideOpenHarness,
	derivePromptFirstLine,
	parseCloseHarnessArgs,
	parseCloseRoomArgs,
	parseCreateArgs,
	parseOpenHarnessArgs,
	parseOpenHarnessResolveArgs,
	parseResolveArgs,
	resolveAgent,
	resolveKind,
	specFromCreateArgs,
} from "./agentRequests.ts";
import type { AgentListing } from "./agents.ts";
import { NO_AGENTS } from "./agents.ts";
import type { DefaultAgents, FolderDefaults } from "./prefs.ts";
import type { Room } from "./types.ts";

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

	// #356: `prompt`/`promptFirstLine` are both optional — absent from
	// `valid` above, so every other case in this describe block already
	// covers "omitted entirely". These cover present/null explicitly.
	it("carries prompt and promptFirstLine through only when given", () => {
		expect(parseCreateArgs({ ...valid, prompt: "fix the thing\n\nsee #1" })).toEqual({
			ok: true,
			value: { ...valid, prompt: "fix the thing\n\nsee #1" },
		});
		expect(parseCreateArgs({ ...valid, promptFirstLine: "fix the thing" })).toEqual({
			ok: true,
			value: { ...valid, promptFirstLine: "fix the thing" },
		});
		expect(
			parseCreateArgs({
				...valid,
				prompt: "fix the thing\n\nsee #1",
				promptFirstLine: "fix the thing",
			}),
		).toEqual({
			ok: true,
			value: {
				...valid,
				prompt: "fix the thing\n\nsee #1",
				promptFirstLine: "fix the thing",
			},
		});
	});

	// Same null-tolerance regression as branch/baseBranch above — Rust's
	// `json!` macro sends an absent `Option<String>` as explicit `null`.
	it("treats null prompt/promptFirstLine the same as absent", () => {
		expect(parseCreateArgs({ ...valid, prompt: null, promptFirstLine: null })).toEqual({
			ok: true,
			value: valid,
		});
	});

	it("rejects a prompt or promptFirstLine that is neither a string nor null", () => {
		expect(parseCreateArgs({ ...valid, prompt: 42 })).toEqual({
			ok: false,
			error: "prompt must be a string",
		});
		expect(parseCreateArgs({ ...valid, promptFirstLine: 42 })).toEqual({
			ok: false,
			error: "promptFirstLine must be a string",
		});
	});
});

describe("derivePromptFirstLine", () => {
	it("is undefined when neither explicit nor prompt is given", () => {
		expect(derivePromptFirstLine(undefined, undefined)).toBeUndefined();
	});

	it("derives the first non-empty trimmed line of prompt when explicit is absent", () => {
		expect(derivePromptFirstLine(undefined, "fix the thing\n\nsee #1")).toBe("fix the thing");
	});

	it("skips leading blank lines to find the first non-empty one", () => {
		expect(derivePromptFirstLine(undefined, "\n  \nfix the thing\nsee #1")).toBe("fix the thing");
	});

	it("is undefined when prompt is only blank lines", () => {
		expect(derivePromptFirstLine(undefined, "\n  \n\t\n")).toBeUndefined();
	});

	it("explicit wins outright over prompt, even a non-blank one", () => {
		expect(derivePromptFirstLine("the real title", "fix the thing\nsee #1")).toBe("the real title");
	});

	it("falls through to prompt when explicit is blank", () => {
		expect(derivePromptFirstLine("   ", "fix the thing")).toBe("fix the thing");
	});

	it("caps at 200 chars, from explicit and from a derived line alike", () => {
		const long = "x".repeat(250);
		expect(derivePromptFirstLine(long, undefined)).toBe("x".repeat(200));
		expect(derivePromptFirstLine(undefined, long)).toBe("x".repeat(200));
	});

	it("trims surrounding whitespace on both paths", () => {
		expect(derivePromptFirstLine("  the real title  ", undefined)).toBe("the real title");
		expect(derivePromptFirstLine(undefined, "  fix the thing  \nsee #1")).toBe("fix the thing");
	});
});

describe("parseCloseRoomArgs", () => {
	const valid = { roomId: "r1", closedBy: { roomId: "s1", harnessId: "h1" } };

	it("accepts a fully-formed request", () => {
		expect(parseCloseRoomArgs(valid)).toEqual({ ok: true, value: valid });
	});

	it("rejects a non-object payload", () => {
		expect(parseCloseRoomArgs("nope")).toEqual({ ok: false, error: "args must be an object" });
	});

	it("rejects a missing or empty roomId", () => {
		expect(parseCloseRoomArgs({ ...valid, roomId: undefined })).toEqual({
			ok: false,
			error: "roomId is required",
		});
		expect(parseCloseRoomArgs({ ...valid, roomId: "" })).toEqual({
			ok: false,
			error: "roomId is required",
		});
	});

	it("rejects a missing or malformed closedBy", () => {
		expect(parseCloseRoomArgs({ ...valid, closedBy: undefined })).toEqual({
			ok: false,
			error: "closedBy must be {roomId, harnessId?}",
		});
		expect(parseCloseRoomArgs({ ...valid, closedBy: { harnessId: "h1" } })).toEqual({
			ok: false,
			error: "closedBy must be {roomId, harnessId?}",
		});
		expect(parseCloseRoomArgs({ ...valid, closedBy: { roomId: "" } })).toEqual({
			ok: false,
			error: "closedBy must be {roomId, harnessId?}",
		});
	});

	it("closedBy.harnessId is optional — omitted entirely is fine", () => {
		expect(parseCloseRoomArgs({ roomId: "r1", closedBy: { roomId: "s1" } })).toEqual({
			ok: true,
			value: { roomId: "r1", closedBy: { roomId: "s1" } },
		});
	});

	// Regression, same shape as create_room's createdBy: Rust's
	// `serde_json::json!` macro sends an absent `Option<String>` as
	// explicit JSON `null`, never as a missing key.
	it("treats explicit null harnessId the same as absent", () => {
		expect(
			parseCloseRoomArgs({ roomId: "r1", closedBy: { roomId: "s1", harnessId: null } }),
		).toEqual({ ok: true, value: { roomId: "r1", closedBy: { roomId: "s1" } } });
	});

	it("rejects a non-string, non-null harnessId", () => {
		expect(parseCloseRoomArgs({ roomId: "r1", closedBy: { roomId: "s1", harnessId: 42 } })).toEqual(
			{ ok: false, error: "closedBy.harnessId must be a string" },
		);
	});
});

describe("decideCloseRoom", () => {
	const room = (over: Partial<Room> = {}): Room => ({
		id: "r1",
		name: "room one",
		task: "do the thing",
		status: "idle",
		badge: 0,
		harnesses: [{ id: "h1", kind: "claude", name: "main", status: "idle", model: "", tokens: "" }],
		activeHarnessId: "h1",
		...over,
	});
	const closedBy = { roomId: "s1", harnessId: "h9" };
	const noDirty = () => [];

	it("refuses an unknown room", () => {
		expect(decideCloseRoom([], "r1", closedBy, noDirty, 100)).toEqual({
			ok: false,
			error: 'not_found: room "r1" not found',
		});
	});

	it("refuses an already-archived room", () => {
		expect(decideCloseRoom([room({ archived: 50 })], "r1", closedBy, noDirty, 100)).toEqual({
			ok: false,
			error: 'archived: room "r1" is already archived',
		});
	});

	it("refuses with the dirty file names when Files buffers are unsaved", () => {
		const dirtyNamesFor = (ids: string[]) => {
			expect(ids).toEqual(["h1"]);
			return ["a.txt", "b.txt"];
		};
		expect(decideCloseRoom([room()], "r1", closedBy, dirtyNamesFor, 100)).toEqual({
			ok: false,
			error: "unsaved_files: a.txt, b.txt",
		});
	});

	it("ok: stamps closedBy with the caller attribution and `now`", () => {
		const r = room();
		expect(decideCloseRoom([r], "r1", closedBy, noDirty, 100)).toEqual({
			ok: true,
			value: { room: r, closedBy: { roomId: "s1", harnessId: "h9", at: 100 } },
		});
	});

	it("ok: closedBy.harnessId omitted when the caller didn't have one", () => {
		expect(decideCloseRoom([room()], "r1", { roomId: "s1" }, noDirty, 100)).toEqual({
			ok: true,
			value: { room: room(), closedBy: { roomId: "s1", at: 100 } },
		});
	});
});

describe("parseOpenHarnessResolveArgs", () => {
	const valid = { roomId: "r1", prompt: false };

	it("accepts a fully-formed request", () => {
		expect(parseOpenHarnessResolveArgs(valid)).toEqual({ ok: true, value: valid });
	});

	it("rejects a non-object payload", () => {
		expect(parseOpenHarnessResolveArgs("nope")).toEqual({
			ok: false,
			error: "args must be an object",
		});
	});

	it("rejects a missing or empty roomId", () => {
		expect(parseOpenHarnessResolveArgs({ ...valid, roomId: undefined })).toEqual({
			ok: false,
			error: "roomId is required",
		});
		expect(parseOpenHarnessResolveArgs({ ...valid, roomId: "" })).toEqual({
			ok: false,
			error: "roomId is required",
		});
	});

	it("carries kind and agent through when given", () => {
		expect(parseOpenHarnessResolveArgs({ ...valid, kind: "claude", agent: "reviewer" })).toEqual({
			ok: true,
			value: { ...valid, kind: "claude", agent: "reviewer" },
		});
	});

	it("rejects a non-string kind or agent", () => {
		expect(parseOpenHarnessResolveArgs({ ...valid, kind: 3 })).toEqual({
			ok: false,
			error: "kind must be a string",
		});
		expect(parseOpenHarnessResolveArgs({ ...valid, agent: 3 })).toEqual({
			ok: false,
			error: "agent must be a string",
		});
	});

	// Same null-tolerance regression as create_room.resolve's — Rust's
	// `json!` macro sends an absent `Option<String>` as explicit `null`.
	it("treats explicit null kind/agent the same as absent", () => {
		expect(parseOpenHarnessResolveArgs({ ...valid, kind: null, agent: null })).toEqual({
			ok: true,
			value: valid,
		});
	});

	it("rejects a non-boolean prompt", () => {
		expect(parseOpenHarnessResolveArgs({ roomId: "r1" })).toEqual({
			ok: false,
			error: "prompt must be a boolean",
		});
		expect(parseOpenHarnessResolveArgs({ ...valid, prompt: "yes" })).toEqual({
			ok: false,
			error: "prompt must be a boolean",
		});
	});
});

describe("parseOpenHarnessArgs", () => {
	const valid = {
		roomId: "r1",
		kind: "claude" as const,
		agent: null,
		createdBy: { roomId: "s1", harnessId: "h1" },
	};

	it("accepts a fully-formed request", () => {
		expect(parseOpenHarnessArgs(valid)).toEqual({ ok: true, value: valid });
	});

	it("rejects a non-object payload", () => {
		expect(parseOpenHarnessArgs("nope")).toEqual({ ok: false, error: "args must be an object" });
	});

	it("rejects a missing or empty roomId", () => {
		expect(parseOpenHarnessArgs({ ...valid, roomId: "" })).toEqual({
			ok: false,
			error: "roomId is required",
		});
	});

	it("rejects an unknown kind", () => {
		expect(parseOpenHarnessArgs({ ...valid, kind: "not-a-kind" })).toEqual({
			ok: false,
			error: 'unknown harness kind "not-a-kind"',
		});
	});

	it("rejects an agent that is neither a string nor null", () => {
		expect(parseOpenHarnessArgs({ ...valid, agent: 42 })).toEqual({
			ok: false,
			error: "agent must be a string or null",
		});
	});

	it("accepts a string agent", () => {
		expect(parseOpenHarnessArgs({ ...valid, agent: "reviewer" })).toEqual({
			ok: true,
			value: { ...valid, agent: "reviewer" },
		});
	});

	it("rejects a missing or malformed createdBy", () => {
		expect(parseOpenHarnessArgs({ ...valid, createdBy: undefined })).toEqual({
			ok: false,
			error: "createdBy must be {roomId, harnessId?}",
		});
		expect(parseOpenHarnessArgs({ ...valid, createdBy: { roomId: "" } })).toEqual({
			ok: false,
			error: "createdBy must be {roomId, harnessId?}",
		});
	});

	it("createdBy.harnessId is optional — omitted entirely is fine", () => {
		expect(parseOpenHarnessArgs({ ...valid, createdBy: { roomId: "s1" } })).toEqual({
			ok: true,
			value: { ...valid, createdBy: { roomId: "s1" } },
		});
	});

	// Regression, same shape as close_room's closedBy: Rust's
	// `serde_json::json!` macro sends an absent `Option<String>` as
	// explicit JSON `null`, never as a missing key.
	it("treats explicit null createdBy.harnessId the same as absent", () => {
		expect(
			parseOpenHarnessArgs({ ...valid, createdBy: { roomId: "s1", harnessId: null } }),
		).toEqual({ ok: true, value: { ...valid, createdBy: { roomId: "s1" } } });
	});

	it("rejects a non-string, non-null createdBy.harnessId", () => {
		expect(parseOpenHarnessArgs({ ...valid, createdBy: { roomId: "s1", harnessId: 42 } })).toEqual({
			ok: false,
			error: "createdBy.harnessId must be a string",
		});
	});
});

describe("decideOpenHarness", () => {
	const room = (over: Partial<Room> = {}): Room => ({
		id: "r1",
		name: "room one",
		task: "do the thing",
		status: "idle",
		badge: 0,
		harnesses: [{ id: "h1", kind: "claude", name: "main", status: "idle", model: "", tokens: "" }],
		activeHarnessId: "h1",
		...over,
	});

	it("refuses an unknown room", () => {
		expect(decideOpenHarness([], "r1")).toEqual({
			ok: false,
			error: 'not_found: room "r1" not found',
		});
	});

	it("refuses an already-archived room", () => {
		expect(decideOpenHarness([room({ archived: 50 })], "r1")).toEqual({
			ok: false,
			error: 'archived: room "r1" is already archived',
		});
	});

	it("ok: hands back the room", () => {
		const r = room();
		expect(decideOpenHarness([r], "r1")).toEqual({ ok: true, value: { room: r } });
	});
});

describe("parseCloseHarnessArgs", () => {
	const valid = { roomId: "r1", harnessId: "h1", closedBy: { roomId: "s1", harnessId: "h9" } };

	it("accepts a fully-formed request", () => {
		expect(parseCloseHarnessArgs(valid)).toEqual({ ok: true, value: valid });
	});

	it("rejects a non-object payload", () => {
		expect(parseCloseHarnessArgs("nope")).toEqual({ ok: false, error: "args must be an object" });
	});

	it("rejects a missing or empty roomId", () => {
		expect(parseCloseHarnessArgs({ ...valid, roomId: "" })).toEqual({
			ok: false,
			error: "roomId is required",
		});
	});

	it("rejects a missing or empty harnessId", () => {
		expect(parseCloseHarnessArgs({ ...valid, harnessId: "" })).toEqual({
			ok: false,
			error: "harnessId is required",
		});
	});

	it("rejects a missing or malformed closedBy", () => {
		expect(parseCloseHarnessArgs({ ...valid, closedBy: undefined })).toEqual({
			ok: false,
			error: "closedBy must be {roomId, harnessId?}",
		});
		expect(parseCloseHarnessArgs({ ...valid, closedBy: { roomId: "" } })).toEqual({
			ok: false,
			error: "closedBy must be {roomId, harnessId?}",
		});
	});

	it("closedBy.harnessId is optional — omitted entirely is fine", () => {
		expect(parseCloseHarnessArgs({ ...valid, closedBy: { roomId: "s1" } })).toEqual({
			ok: true,
			value: { ...valid, closedBy: { roomId: "s1" } },
		});
	});

	// Regression: Rust's `serde_json::json!` macro sends an absent
	// `Option<String>` as explicit JSON `null`, never as a missing key.
	it("treats explicit null closedBy.harnessId the same as absent", () => {
		expect(
			parseCloseHarnessArgs({ ...valid, closedBy: { roomId: "s1", harnessId: null } }),
		).toEqual({ ok: true, value: { ...valid, closedBy: { roomId: "s1" } } });
	});

	it("rejects a non-string, non-null closedBy.harnessId", () => {
		expect(parseCloseHarnessArgs({ ...valid, closedBy: { roomId: "s1", harnessId: 42 } })).toEqual({
			ok: false,
			error: "closedBy.harnessId must be a string",
		});
	});
});

describe("decideCloseHarness", () => {
	const room = (over: Partial<Room> = {}): Room => ({
		id: "r1",
		name: "room one",
		task: "do the thing",
		status: "idle",
		badge: 0,
		harnesses: [
			{ id: "h1", kind: "claude", name: "main", status: "idle", model: "", tokens: "" },
			{ id: "h2", kind: "opencode", name: "second", status: "idle", model: "", tokens: "" },
		],
		activeHarnessId: "h1",
		...over,
	});
	const noPhase = () => undefined;
	const noDirty = () => [];

	it("refuses an unknown room", () => {
		expect(decideCloseHarness([], "r1", "h1", noPhase, noDirty)).toEqual({
			ok: false,
			error: 'not_found: room "r1" not found',
		});
	});

	it("refuses an unknown harness", () => {
		expect(decideCloseHarness([room()], "r1", "ghost", noPhase, noDirty)).toEqual({
			ok: false,
			error: 'not_found: harness "ghost" not found',
		});
	});

	it("refuses the room's only harness", () => {
		const solo = room({
			harnesses: [
				{ id: "h1", kind: "claude", name: "main", status: "idle", model: "", tokens: "" },
			],
		});
		expect(decideCloseHarness([solo], "r1", "h1", noPhase, noDirty)).toEqual({
			ok: false,
			error: 'last_harness: "h1" is the only harness in room "r1" — use close_room instead',
		});
	});

	it("refuses a harness with an open permission dialog", () => {
		expect(decideCloseHarness([room()], "r1", "h1", () => "permission", noDirty)).toEqual({
			ok: false,
			error: 'permission_open: harness "h1" has an open permission dialog',
		});
	});

	it("refuses with the dirty file names when Files buffers are unsaved", () => {
		const dirtyNamesFor = (hid: string) => {
			expect(hid).toBe("h1");
			return ["a.txt", "b.txt"];
		};
		expect(decideCloseHarness([room()], "r1", "h1", noPhase, dirtyNamesFor)).toEqual({
			ok: false,
			error: "unsaved_files: a.txt, b.txt",
		});
	});

	it("ok: reports the phase from before the close, defaulting to unknown", () => {
		const r = room();
		expect(decideCloseHarness([r], "r1", "h1", noPhase, noDirty)).toEqual({
			ok: true,
			value: { room: r, harness: r.harnesses[0], phase: "unknown" },
		});
		expect(decideCloseHarness([r], "r1", "h1", () => "running", noDirty)).toEqual({
			ok: true,
			value: { room: r, harness: r.harnesses[0], phase: "running" },
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
