import { describe, expect, it } from "vitest";
import { launchOf, runsItsTool, spawnArgv } from "./harnessLaunch.ts";
import type { LaunchMode } from "./types.ts";

describe("harnessLaunch", () => {
	it("launchOf defaults to the tool", () => {
		expect(launchOf({})).toEqual({ mode: "tool" });
		expect(launchOf({ launch: { mode: "shell", argv: ["sh"] } })).toEqual({
			mode: "shell",
			argv: ["sh"],
		});
	});

	it("spawnArgv is cmd for the tool and the shell argv in shell mode", () => {
		expect(spawnArgv({ cmd: ["claude"] })).toEqual(["claude"]);
		expect(spawnArgv({ cmd: ["claude"], launch: { mode: "tool" } })).toEqual(["claude"]);
		expect(spawnArgv({ cmd: ["claude"], launch: { mode: "shell", argv: ["bash"] } })).toEqual([
			"bash",
		]);
		expect(spawnArgv({})).toBeUndefined();
	});

	it("runsItsTool is false only in the shell without a claim (#763)", () => {
		const shell: LaunchMode = { mode: "shell", argv: ["bash"] };
		expect(runsItsTool({})).toBe(true);
		expect(runsItsTool({ launch: { mode: "tool" } })).toBe(true);
		expect(runsItsTool({ launch: shell })).toBe(false);
		expect(runsItsTool({ launch: shell, shellClaim: { sessionId: "s" } })).toBe(true);
	});
});
