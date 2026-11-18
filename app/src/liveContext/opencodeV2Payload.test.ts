// #678: opencode v2 spellings in the payload-reading layer.

import { describe, expect, it } from "vitest";
import { canonicalTool, inputFilePath } from "./payload.ts";
import { attributeHunks } from "./review.ts";
import type { HarnessAction } from "./store.ts";
import { harnessTool } from "./toolShared.ts";

describe("harnessTool aliases", () => {
	it("reads v2 shell as bash and subagent as task", () => {
		expect(harnessTool({ tool: "shell" })).toBe("bash");
		expect(harnessTool({ tool: "subagent" })).toBe("task");
		expect(canonicalTool("Shell")).toBe("bash");
	});

	it("leaves other names alone", () => {
		expect(harnessTool({ tool: "Bash" })).toBe("bash");
		expect(harnessTool({ tool: "read" })).toBe("read");
		expect(harnessTool({})).toBe("");
	});
});

describe("inputFilePath", () => {
	it("accepts file_path, filePath and path", () => {
		expect(inputFilePath({ file_path: "/a" })).toBe("/a");
		expect(inputFilePath({ filePath: "/b" })).toBe("/b");
		expect(inputFilePath({ path: "/c" })).toBe("/c");
		expect(inputFilePath(undefined)).toBeUndefined();
	});
});

describe("grep/glob search roots", () => {
	it("keep reading the pattern, not the path", () => {
		// SearchRow reads only input.pattern; the path helper is never used for it.
		expect(harnessTool({ tool: "Grep", input: { pattern: "x", path: "src" } })).toBe("grep");
		expect(harnessTool({ tool: "glob", input: { pattern: "*.rs", path: "src" } })).toBe("glob");
	});
});

describe("attributeHunks with input.path", () => {
	it("attributes an edit that names its file only through input.path", () => {
		const action: HarnessAction = {
			harnessId: "h-1",
			kind: "patch",
			payload: JSON.stringify({
				tool: "edit",
				input: { path: "C:\\wt\\src\\a.ts" },
				patch_info: { diff: "@@ -1,1 +1,1 @@\n-a\n+b\n" },
			}),
		} as HarnessAction;
		const file = {
			path: "src/a.ts",
			harnessId: undefined,
			hunks: [{ newStart: 1, newLines: 1 }],
		} as unknown as Parameters<typeof attributeHunks>[1];
		expect(attributeHunks([action], file)).toEqual(["h-1"]);
	});
});
