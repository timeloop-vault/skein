import { describe, expect, it } from "vitest";
import {
	cmdForKind,
	isOpencodeV2,
	opencodeSpawnPlan,
	opencodeV2Argv,
	resumeCmd,
} from "./harnessCmd.ts";
import type { Harness, OpencodeVersion } from "./types.ts";

const v = (major: number): OpencodeVersion => ({ major, version: `${major}.0.0` });

const oc = (over: Partial<Harness> = {}): Harness => ({
	id: "h1",
	kind: "opencode",
	name: "H",
	status: "idle",
	model: "",
	tokens: "",
	...over,
});

describe("opencode v2 spawn plan (#676)", () => {
	it("v2 argv shape has no --agent", () => {
		expect(opencodeV2Argv("http://127.0.0.1:5000", "ses_1")).toEqual([
			"opencode",
			"--server",
			"http://127.0.0.1:5000",
			"--session",
			"ses_1",
		]);
	});

	it("isOpencodeV2 is major >= 2 only", () => {
		expect(isOpencodeV2(v(2))).toBe(true);
		expect(isOpencodeV2(v(3))).toBe(true);
		expect(isOpencodeV2(v(1))).toBe(false);
		expect(isOpencodeV2(null)).toBe(false);
		expect(isOpencodeV2(undefined)).toBe(false);
	});

	// The plan never rewrites argv: v1 and unknown versions keep exactly what
	// cmdForKind / resumeCmd produced.
	const cmds = [
		cmdForKind("opencode", ["sh"], undefined, 4100),
		cmdForKind("opencode", ["sh"], undefined, 4100, "build"),
		resumeCmd(oc({ cmd: ["opencode"], sessionId: "ses_1" }), 4100),
		resumeCmd(oc({ cmd: ["opencode"] }), 4100),
		resumeCmd(oc({ cmd: ["opencode"], sessionId: "ses_1", agent: "plan" }), 4100),
	];

	it("v1 and null versions plan v1 for every stored argv shape", () => {
		for (const cmd of cmds) {
			expect(opencodeSpawnPlan(cmd, "opencode", v(1))).toEqual({ mode: "v1" });
			expect(opencodeSpawnPlan(cmd, "opencode", null)).toEqual({ mode: "v1" });
			expect(opencodeSpawnPlan(cmd, "opencode", undefined)).toEqual({ mode: "v1" });
		}
	});

	it("v2 plans v2 for an opencode-owned argv", () => {
		for (const cmd of cmds)
			expect(opencodeSpawnPlan(cmd, "opencode", v(2))).toEqual({ mode: "v2" });
	});

	it("a shell cmd or a non-opencode kind is never v2", () => {
		expect(opencodeSpawnPlan(["pwsh.exe"], "opencode", v(2))).toEqual({ mode: "v1" });
		expect(opencodeSpawnPlan(["claude"], "claude", v(2))).toEqual({ mode: "v1" });
	});
});
