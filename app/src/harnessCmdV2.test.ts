import { describe, expect, it } from "vitest";
import { isOpencodeV2 } from "./harnessCmd.ts";
import type { OpencodeVersion } from "./types.ts";

const v = (major: number): OpencodeVersion => ({ major, version: `${major}.0.0` });

describe("isOpencodeV2 (#676)", () => {
	it("isOpencodeV2 is major >= 2 only", () => {
		expect(isOpencodeV2(v(2))).toBe(true);
		expect(isOpencodeV2(v(3))).toBe(true);
		expect(isOpencodeV2(v(1))).toBe(false);
		expect(isOpencodeV2(null)).toBe(false);
		expect(isOpencodeV2(undefined)).toBe(false);
	});
});
