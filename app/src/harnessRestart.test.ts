import { describe, expect, it } from "vitest";
import type { ComposerDraft } from "./composerDraft.ts";
import { HARNESS_KINDS } from "./data.tsx";
import type { ActivityPhase } from "./harnessActivityTypes.ts";
import { type CanRestartInput, canRestart, restartArgv } from "./harnessRestart.ts";
import type { Harness, HarnessKind } from "./types.ts";

const input = (over: Partial<CanRestartInput> = {}): CanRestartInput => ({
	kind: "claude",
	capabilities: HARNESS_KINDS.claude.capabilities,
	phase: "waiting",
	mailHeld: false,
	draft: { kind: "clean" },
	...over,
});

const forKind = (kind: HarnessKind): Partial<CanRestartInput> => ({
	kind,
	capabilities: HARNESS_KINDS[kind].capabilities,
});

const reasonOf = (r: ReturnType<typeof canRestart>) => (r.ok ? null : r.reason);

describe("canRestart: capability", () => {
	for (const kind of Object.keys(HARNESS_KINDS) as HarnessKind[]) {
		const c = HARNESS_KINDS[kind].capabilities;
		const expected = c.pty && c.resume;
		it(`${kind}: ${expected ? "allowed" : "refused"}`, () => {
			const r = canRestart(input(forKind(kind)));
			expect(r.ok).toBe(expected);
			if (!expected) expect(reasonOf(r)).toContain("can't be restarted");
		});
	}

	it("claude and opencode are restartable, shell/files are not", () => {
		expect(canRestart(input(forKind("claude"))).ok).toBe(true);
		expect(canRestart(input(forKind("opencode"))).ok).toBe(true);
		expect(canRestart(input(forKind("byoh"))).ok).toBe(false);
		expect(canRestart(input(forKind("files"))).ok).toBe(false);
	});

	it("each capability alone is not enough", () => {
		const base = HARNESS_KINDS.claude.capabilities;
		expect(canRestart(input({ capabilities: { ...base, pty: false } })).ok).toBe(false);
		expect(canRestart(input({ capabilities: { ...base, resume: false } })).ok).toBe(false);
	});
});

describe("canRestart: phase", () => {
	const table: Array<[ActivityPhase | null, boolean, string | null]> = [
		["waiting", true, null],
		["idle", true, null],
		["running", false, "busy"],
		["permission", false, "permission dialog"],
		["spawning", false, "starting"],
		["exited", false, "exited"],
		[null, false, "no live process"],
	];
	for (const [phase, ok, fragment] of table) {
		it(`${String(phase)} -> ${ok ? "ok" : "refused"}`, () => {
			const r = canRestart(input({ phase }));
			expect(r.ok).toBe(ok);
			if (fragment) expect(reasonOf(r)).toContain(fragment);
		});
	}
});

describe("canRestart: mail and draft", () => {
	it("held mail refuses", () => {
		const r = canRestart(input({ mailHeld: true }));
		expect(reasonOf(r)).toContain("held mail");
	});

	const drafts: Array<[ComposerDraft, boolean]> = [
		[{ kind: "clean" }, true],
		[{ kind: "typed", chars: 3 }, false],
		[{ kind: "unknown" }, false],
	];
	for (const [draft, ok] of drafts) {
		it(`draft ${draft.kind} -> ${ok ? "ok" : "refused"}`, () => {
			const r = canRestart(input({ draft }));
			expect(r.ok).toBe(ok);
			if (!ok) expect(reasonOf(r)).toContain("unsent text");
		});
	}
});

describe("canRestart: precedence", () => {
	const everything = {
		phase: "running" as const,
		mailHeld: true,
		draft: { kind: "typed", chars: 1 } as ComposerDraft,
	};
	it("capability beats phase", () => {
		expect(reasonOf(canRestart(input({ ...everything, ...forKind("byoh") })))).toContain(
			"can't be restarted",
		);
	});
	it("phase beats mail and draft", () => {
		expect(reasonOf(canRestart(input(everything)))).toContain("busy");
	});
	it("mail beats draft", () => {
		expect(reasonOf(canRestart(input({ ...everything, phase: "waiting" })))).toContain("held mail");
	});
	it("draft last", () => {
		expect(
			reasonOf(canRestart(input({ ...everything, phase: "idle", mailHeld: false }))),
		).toContain("unsent text");
	});
});

describe("restartArgv", () => {
	const h = (over: Partial<Harness>): Harness => ({
		id: "h1",
		kind: "claude",
		name: "x",
		status: "idle",
		model: "",
		tokens: "",
		cmd: ["claude", "--session-id", "abc"],
		...over,
	});
	it("claude resumes by session id", () => {
		expect(restartArgv(h({ sessionId: "abc" }))).toEqual(["claude", "--resume", "abc"]);
	});
	it("opencode bakes in the new port", () => {
		expect(restartArgv(h({ kind: "opencode", cmd: ["opencode"], sessionId: "s" }), 4123)).toEqual([
			"opencode",
			"--port",
			"4123",
			"--hostname",
			"127.0.0.1",
			"--session",
			"s",
		]);
	});
});
