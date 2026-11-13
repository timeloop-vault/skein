import { describe, expect, it } from "vitest";
import { launchOf, spawnArgv } from "./harnessLaunch.ts";

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
});
