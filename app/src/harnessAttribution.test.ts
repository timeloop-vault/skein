import { describe, expect, it } from "vitest";
import { resolveHarnessKind } from "./harnessAttribution.ts";
import type { HarnessKind } from "./types.ts";

const current = new Map<string, HarnessKind>([["h1", "opencode"]]);

describe("resolveHarnessKind", () => {
	it("stored valid kind wins over a current harness of another kind", () => {
		expect(resolveHarnessKind("claude", "h1", current)).toBe("claude");
	});
	it("null stored + current harness gives the current kind", () => {
		expect(resolveHarnessKind(null, "h1", current)).toBe("opencode");
		expect(resolveHarnessKind(undefined, "h1", [{ id: "h1", kind: "files" }])).toBe("files");
	});
	it("null stored + unknown id is unknown", () => {
		expect(resolveHarnessKind(null, "gone", current)).toBeNull();
	});
	it("unrecognised stored + unknown id is unknown", () => {
		expect(resolveHarnessKind("nonsense", "gone", current)).toBeNull();
	});
	it("unrecognised stored + current harness gives the current kind", () => {
		expect(resolveHarnessKind("nonsense", "h1", current)).toBe("opencode");
	});
});
