import { beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
	markSpawned: vi.fn(),
	adoptionForget: vi.fn(),
	shellForget: vi.fn(),
}));
vi.mock("./claudeVersionStore.ts", () => ({ claudeVersionStore: { markSpawned: m.markSpawned } }));
vi.mock("./startupAdoption.ts", () => ({ startupAdoption: { forget: m.adoptionForget } }));
vi.mock("./shellClaim.ts", () => ({ shellClaim: { forget: m.shellForget } }));

import type { EnsureResult } from "./harnessEnsure.ts";
import { spawnedProfile } from "./spawnedEnvProfile.ts";
import { type ApplyEnsure, applyEnsureResult, forgetViewState } from "./terminalEnsureApply.ts";

const result = (over: Partial<EnsureResult> = {}): EnsureResult => ({
	started: true,
	id: "p1",
	injected: true,
	hermesHooks: false,
	mouseClicksDisabled: false,
	envProfile: "work",
	sidecarGeneration: null,
	opencodeVersion: null,
	sessionId: null,
	exited: false,
	...over,
});

const apply = (over: Partial<ApplyEnsure> = {}): ApplyEnsure => ({
	result: result(),
	harnessId: "h1",
	harnessKind: "claude",
	sessionId: "s0",
	opencodePort: 4001,
	spawnStartedAt: 1234,
	opencodeClaim: undefined,
	onSessionCaptured: undefined,
	...over,
});

beforeEach(() => {
	for (const f of Object.values(m)) f.mockReset();
});

describe("applyEnsureResult", () => {
	it("a started process gets fresh-process bookkeeping", () => {
		applyEnsureResult(apply());
		expect(m.markSpawned).toHaveBeenCalledWith("h1", 1234);
		expect(m.adoptionForget).toHaveBeenCalledWith("h1");
		expect(spawnedProfile("h1")).toBe("work");
	});

	it("a re-attach keeps every row as its own (spawnedAt 0) and forgets no adoption", () => {
		applyEnsureResult(apply({ result: result({ started: false }) }));
		expect(m.markSpawned).toHaveBeenCalledWith("h1", 0);
		expect(m.adoptionForget).not.toHaveBeenCalled();
	});

	it("an already-exited result registers nothing but the sink-independent record", () => {
		// exited results are handled by the caller before this is reached; this
		// pins that applying one does not touch the claim or session sinks.
		const setOpencodeVersion = vi.fn();
		const onSessionCaptured = vi.fn();
		applyEnsureResult(
			apply({
				result: result({ started: false, exited: true, exitCode: 1 }),
				opencodeClaim: { setOpencodeVersion } as never,
				onSessionCaptured,
			}),
		);
		expect(onSessionCaptured).not.toHaveBeenCalled();
		expect(m.adoptionForget).not.toHaveBeenCalled();
	});

	it("reports the captured session id and the opencode version to the sinks", () => {
		const setOpencodeVersion = vi.fn();
		const onSessionCaptured = vi.fn();
		const out = applyEnsureResult(
			apply({
				harnessKind: "opencode",
				result: result({ sessionId: "s1", opencodeVersion: "v2" as never }),
				opencodeClaim: { setOpencodeVersion } as never,
				onSessionCaptured,
			}),
		);
		expect(onSessionCaptured).toHaveBeenCalledWith("s1");
		expect(setOpencodeVersion).toHaveBeenCalledWith("v2");
		expect(out).toEqual({ sidecarGeneration: undefined, port: 4001 });
	});

	it("attaches by sidecar generation (no port) when the daemon started one", () => {
		const out = applyEnsureResult(apply({ result: result({ sidecarGeneration: 3 }) }));
		expect(out).toEqual({ sidecarGeneration: 3, port: undefined });
	});
});

describe("forgetViewState", () => {
	it("drops the per-harness view state", () => {
		forgetViewState("h1");
		expect(m.shellForget).toHaveBeenCalledWith("h1");
		expect(m.adoptionForget).toHaveBeenCalledWith("h1");
		expect(spawnedProfile("h1")).toBeUndefined();
	});
});
